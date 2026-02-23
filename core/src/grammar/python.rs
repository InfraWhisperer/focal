use tree_sitter::{Language, Node, Tree};

use super::{ExtractedReference, ExtractedSymbol, Grammar, SymbolKind};

pub struct PythonGrammar;

impl Grammar for PythonGrammar {
    fn language(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn file_extensions(&self) -> &[&str] {
        &["py", "pyi"]
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
            "function_definition" => {
                if let Some(sym) = extract_function(&child, source) {
                    out.push(sym);
                }
            }
            "class_definition" => {
                if let Some(sym) = extract_class(&child, source) {
                    out.push(sym);
                }
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

    // Extract methods from the class body block
    let mut children = Vec::new();
    if let Some(body_node) = node.child_by_field_name("body") {
        let mut cursor = body_node.walk();
        for child in body_node.children(&mut cursor) {
            if child.kind() == "function_definition" {
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

/// Build signature from everything before the body block.
/// For Python, body is a `block` child. The signature includes `def name(params) -> type:`.
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

// ---------------------------------------------------------------------------
// Reference extraction
// ---------------------------------------------------------------------------

fn collect_references(root: &Node, source: &[u8], refs: &mut Vec<ExtractedReference>) {
    let mut stack: Vec<Node> = vec![*root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call" {
            if let Some(callee) = extract_callee(&node, source) {
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

/// Extract callee from a `call` node.
/// Handles `foo()` and `obj.method()`.
fn extract_callee(node: &Node, source: &[u8]) -> Option<String> {
    let func_node = node.child_by_field_name("function")?;
    match func_node.kind() {
        "identifier" => Some(node_text(&func_node, source)),
        "attribute" => {
            // e.g. os.listdir — take the last identifier (the attribute name)
            let mut cursor = func_node.walk();
            let children: Vec<Node> = func_node.children(&mut cursor).collect();
            // The attribute name is the last identifier child
            children
                .iter()
                .rev()
                .find(|c| c.kind() == "identifier")
                .map(|n| node_text(n, source))
        }
        _ => Some(node_text(&func_node, source)),
    }
}

/// Collect import statements as import references.
fn collect_import_references(root: &Node, source: &[u8], refs: &mut Vec<ExtractedReference>) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" | "import_from_statement" => {
                let text = node_text(&child, source);
                refs.push(ExtractedReference {
                    from_symbol: String::new(),
                    to_name: text,
                    kind: "imports".to_string(),
                });
            }
            _ => {}
        }
    }
}

/// Walk up to find the nearest enclosing function_definition.
fn find_enclosing_function(node: &Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == "function_definition" {
            let name_node = n.child_by_field_name("name")?;
            return Some(node_text(&name_node, source));
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
