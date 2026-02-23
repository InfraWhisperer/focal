use tree_sitter::{Language, Node, Tree};

use super::{ExtractedReference, ExtractedSymbol, Grammar, SymbolKind};

pub struct RustGrammar;

impl Grammar for RustGrammar {
    fn language(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
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
            "function_item" => {
                if let Some(sym) = extract_function(&child, source) {
                    out.push(sym);
                }
            }
            "struct_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Struct) {
                    out.push(sym);
                }
            }
            "enum_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Enum) {
                    out.push(sym);
                }
            }
            "trait_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Trait) {
                    out.push(sym);
                }
            }
            "const_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Const) {
                    out.push(sym);
                }
            }
            "static_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Const) {
                    out.push(sym);
                }
            }
            "type_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::TypeAlias) {
                    out.push(sym);
                }
            }
            "mod_item" => {
                if let Some(sym) = extract_named_symbol(&child, source, SymbolKind::Module) {
                    out.push(sym);
                }
            }
            "impl_item" => {
                extract_impl(&child, source, out);
            }
            _ => {}
        }
    }
}

/// Extract a top-level `fn` item as a Function symbol.
fn extract_function(node: &Node, source: &[u8]) -> Option<ExtractedSymbol> {
    let name = find_child_by_kind(node, "identifier")
        .map(|n| node_text(&n, source))?;
    let body_node = find_child_by_kind(node, "block");
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

/// Extract a named symbol (struct, enum, trait, const, type alias, module).
/// For structs/enums/traits the name is a `type_identifier` child.
/// For const/static/type items the name is an `identifier` or `type_identifier` child.
fn extract_named_symbol(
    node: &Node,
    source: &[u8],
    kind: SymbolKind,
) -> Option<ExtractedSymbol> {
    // Try type_identifier first (struct, enum, trait, type alias), then identifier (const, static, mod)
    let name = find_child_by_kind(node, "type_identifier")
        .or_else(|| find_child_by_kind(node, "identifier"))
        .map(|n| node_text(&n, source))?;
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

/// Walk an `impl_item`'s `declaration_list` and extract methods.
/// Methods are prefixed with the impl type name (e.g. `Config::new`) to
/// disambiguate identically-named methods across different types.
fn extract_impl(node: &Node, source: &[u8], out: &mut Vec<ExtractedSymbol>) {
    let type_name = find_child_by_kind(node, "type_identifier")
        .map(|n| node_text(&n, source));

    let decl_list = match find_child_by_kind(node, "declaration_list") {
        Some(d) => d,
        None => return,
    };
    let mut cursor = decl_list.walk();
    for child in decl_list.children(&mut cursor) {
        if child.kind() == "function_item" {
            let raw_name = match find_child_by_kind(&child, "identifier") {
                Some(n) => node_text(&n, source),
                None => continue,
            };
            let name = match &type_name {
                Some(t) => format!("{t}::{raw_name}"),
                None => raw_name,
            };
            let body_node = find_child_by_kind(&child, "block");
            let signature = extract_signature(&child, &body_node, source);
            let body = node_text(&child, source);
            out.push(ExtractedSymbol {
                name,
                kind: SymbolKind::Method,
                signature,
                body,
                start_line: child.start_position().row + 1,
                end_line: child.end_position().row + 1,
                children: Vec::new(),
            });
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

/// Walk the entire tree collecting call references.
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
        // Also handle macro invocations like println!(), vec!(), etc.
        if node.kind() == "macro_invocation" {
            if let Some(name_node) = find_child_by_kind(&node, "identifier") {
                let callee = node_text(&name_node, source);
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

/// Extract the callee name from a `call_expression`.
/// Handles `foo()`, `Foo::bar()`, `self.method()`.
fn extract_callee(node: &Node, source: &[u8]) -> Option<String> {
    let func_node = node.child_by_field_name("function")?;
    match func_node.kind() {
        "identifier" => Some(node_text(&func_node, source)),
        "scoped_identifier" => {
            // e.g. String::from — take the last identifier
            let mut last = None;
            let mut cursor = func_node.walk();
            for child in func_node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    last = Some(node_text(&child, source));
                }
            }
            last
        }
        "field_expression" => {
            // e.g. self.method — take the field_identifier
            find_child_by_kind(&func_node, "field_identifier")
                .map(|n| node_text(&n, source))
        }
        _ => Some(node_text(&func_node, source)),
    }
}

/// Collect `use_declaration` nodes as import references.
fn collect_import_references(root: &Node, source: &[u8], refs: &mut Vec<ExtractedReference>) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "use_declaration" {
            let text = node_text(&child, source);
            refs.push(ExtractedReference {
                from_symbol: String::new(),
                to_name: text,
                kind: "imports".to_string(),
            });
        }
    }
}

/// Walk up from a node to find the nearest enclosing function_item and return its name.
fn find_enclosing_function(node: &Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == "function_item" {
            return find_child_by_kind(&n, "identifier").map(|id| node_text(&id, source));
        }
        current = n.parent();
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
