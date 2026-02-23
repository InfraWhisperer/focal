use tree_sitter::{Language, Node, Tree};

use super::{ExtractedReference, ExtractedSymbol, Grammar, SymbolKind};

pub struct TypeScriptGrammar;
pub struct TsxGrammar;

impl Grammar for TypeScriptGrammar {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn file_extensions(&self) -> &[&str] {
        &["ts", "js"]
    }

    fn extract_symbols(&self, source: &[u8], tree: &Tree) -> Vec<ExtractedSymbol> {
        let root = tree.root_node();
        let mut symbols = Vec::new();
        extract_top_level_symbols(&root, source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &[u8], tree: &Tree) -> Vec<ExtractedReference> {
        let root = tree.root_node();
        let mut refs = Vec::new();
        collect_references(&root, source, &mut refs);
        collect_import_references(&root, source, &mut refs);
        refs
    }
}

impl Grammar for TsxGrammar {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    }

    fn file_extensions(&self) -> &[&str] {
        &["tsx", "jsx"]
    }

    fn extract_symbols(&self, source: &[u8], tree: &Tree) -> Vec<ExtractedSymbol> {
        let root = tree.root_node();
        let mut symbols = Vec::new();
        extract_top_level_symbols(&root, source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &[u8], tree: &Tree) -> Vec<ExtractedReference> {
        let root = tree.root_node();
        let mut refs = Vec::new();
        collect_references(&root, source, &mut refs);
        collect_import_references(&root, source, &mut refs);
        refs
    }
}

// ---------------------------------------------------------------------------
// Symbol extraction
// ---------------------------------------------------------------------------

fn extract_top_level_symbols(node: &Node, source: &[u8], out: &mut Vec<ExtractedSymbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(sym) = extract_function(&child, source) {
                    out.push(sym);
                }
            }
            "class_declaration" => {
                if let Some(sym) = extract_class(&child, source) {
                    out.push(sym);
                }
            }
            "interface_declaration" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Interface) {
                    out.push(sym);
                }
            }
            "type_alias_declaration" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::TypeAlias) {
                    out.push(sym);
                }
            }
            "lexical_declaration" => {
                extract_const_declaration(&child, source, out);
            }
            "export_statement" => {
                // Unwrap export and process the inner declaration
                extract_top_level_symbols(&child, source, out);
            }
            _ => {}
        }
    }
}

fn extract_function(node: &Node, source: &[u8]) -> Option<ExtractedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source);
    let body_node = node.child_by_field_name("body");
    let signature = extract_signature(node, &body_node, source);
    let body = node_text(node, source);
    Some(ExtractedSymbol {
        name,
        kind: SymbolKind::Function,
        signature,
        body,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        children: Vec::new(),
    })
}

fn extract_class(node: &Node, source: &[u8]) -> Option<ExtractedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source);
    let body = node_text(node, source);

    // Extract methods as children
    let mut children = Vec::new();
    if let Some(class_body) = node.child_by_field_name("body") {
        let mut cursor = class_body.walk();
        for child in class_body.children(&mut cursor) {
            if child.kind() == "method_definition" {
                if let Some(method) = extract_method(&child, source) {
                    children.push(method);
                }
            }
        }
    }

    let body_node = node.child_by_field_name("body");
    let signature = extract_signature(node, &body_node, source);

    Some(ExtractedSymbol {
        name,
        kind: SymbolKind::Class,
        signature,
        body,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        children,
    })
}

fn extract_method(node: &Node, source: &[u8]) -> Option<ExtractedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source);
    let body_node = node.child_by_field_name("body");
    let signature = extract_signature(node, &body_node, source);
    let body = node_text(node, source);
    Some(ExtractedSymbol {
        name,
        kind: SymbolKind::Method,
        signature,
        body,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        children: Vec::new(),
    })
}

fn extract_named_symbol(
    node: &Node,
    source: &[u8],
    kind: SymbolKind,
) -> Option<ExtractedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source);
    let body = node_text(node, source);
    let signature = extract_declaration_line(&body);
    Some(ExtractedSymbol {
        name,
        kind,
        signature,
        body,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        children: Vec::new(),
    })
}

/// Extract top-level `const` declarations as Const symbols.
/// `lexical_declaration` contains `variable_declarator` children.
fn extract_const_declaration(node: &Node, source: &[u8], out: &mut Vec<ExtractedSymbol>) {
    // Only process if it starts with `const`
    if find_child_by_kind(node, "const").is_none() {
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source);
                let body = node_text(node, source);
                let signature = extract_declaration_line(&body);
                out.push(ExtractedSymbol {
                    name,
                    kind: SymbolKind::Const,
                    signature,
                    body,
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    children: Vec::new(),
                });
            }
        }
    }
}

/// Build a signature from everything before the body block.
fn extract_signature(node: &Node, body_node: &Option<Node>, source: &[u8]) -> String {
    match body_node {
        Some(body) => {
            let start = node.start_byte();
            let end = body.start_byte();
            let raw = &source[start..end];
            String::from_utf8_lossy(raw).trim().to_string()
        }
        None => node_text(node, source),
    }
}

/// Extract the declaration line: everything before the first `{`, trimmed.
/// For single-line items (const, type alias), returns the full text.
fn extract_declaration_line(body: &str) -> String {
    match body.find('{') {
        Some(idx) => body[..idx].trim().to_string(),
        None => body.trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Reference extraction
// ---------------------------------------------------------------------------

fn collect_references(root: &Node, source: &[u8], refs: &mut Vec<ExtractedReference>) {
    let mut stack: Vec<Node> = vec![*root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(callee) = extract_callee(&node, source) {
                let from = find_enclosing_function(&node, source).unwrap_or_default();
                refs.push(ExtractedReference {
                    from_symbol: from,
                    to_name: callee,
                    kind: "calls".to_string(),
                });
            }
        }
        if node.kind() == "new_expression" {
            if let Some(callee) = extract_new_callee(&node, source) {
                let from = find_enclosing_function(&node, source).unwrap_or_default();
                refs.push(ExtractedReference {
                    from_symbol: from,
                    to_name: callee,
                    kind: "calls".to_string(),
                });
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

/// Extract callee from a `call_expression`.
/// Handles `foo()` and `obj.method()`.
fn extract_callee(node: &Node, source: &[u8]) -> Option<String> {
    let func_node = node.child_by_field_name("function")?;
    match func_node.kind() {
        "identifier" => Some(node_text(&func_node, source)),
        "member_expression" => {
            // e.g. service.getUser — take the property
            find_child_by_kind(&func_node, "property_identifier")
                .map(|n| node_text(&n, source))
        }
        _ => Some(node_text(&func_node, source)),
    }
}

/// Extract the constructor name from a `new_expression` (e.g. `new Map()`).
fn extract_new_callee(node: &Node, source: &[u8]) -> Option<String> {
    // The first identifier child after `new` is the class name
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(&child, source));
        }
    }
    None
}

/// Collect import statements as import references.
fn collect_import_references(root: &Node, source: &[u8], refs: &mut Vec<ExtractedReference>) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_statement" {
            let text = node_text(&child, source);
            refs.push(ExtractedReference {
                from_symbol: String::new(),
                to_name: text,
                kind: "imports".to_string(),
            });
        }
    }
}

/// Walk up to find the nearest enclosing function or method.
fn find_enclosing_function(node: &Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        match n.kind() {
            "function_declaration" => {
                let name_node = n.child_by_field_name("name")?;
                return Some(node_text(&name_node, source));
            }
            "method_definition" => {
                let name_node = n.child_by_field_name("name")?;
                return Some(node_text(&name_node, source));
            }
            _ => current = n.parent(),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_text(node: &Node, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or("").to_string()
}

fn find_child_by_kind<'a>(node: &'a Node, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor)
        .find(|child| child.kind() == kind);
    result
}
