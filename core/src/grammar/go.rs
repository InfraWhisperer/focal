use tree_sitter::{Language, Node, Tree};

use super::{ExtractedReference, ExtractedSymbol, Grammar, SymbolKind};

pub struct GoGrammar;

impl Grammar for GoGrammar {
    fn language(&self) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn file_extensions(&self) -> &[&str] {
        &["go"]
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
        refs
    }
}

// ---------------------------------------------------------------------------
// Symbol extraction
// ---------------------------------------------------------------------------

fn extract_top_level_symbols(
    node: &Node,
    source: &[u8],
    out: &mut Vec<ExtractedSymbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(sym) = extract_function(&child, source) {
                    out.push(sym);
                }
            }
            "method_declaration" => {
                if let Some(sym) = extract_method(&child, source) {
                    out.push(sym);
                }
            }
            "type_declaration" => {
                extract_type_decl(&child, source, out);
            }
            "const_declaration" => {
                extract_const_or_var(&child, source, SymbolKind::Const, "const_spec", out);
            }
            "var_declaration" => {
                extract_const_or_var(&child, source, SymbolKind::Const, "var_spec", out);
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

fn extract_type_decl(
    node: &Node,
    source: &[u8],
    out: &mut Vec<ExtractedSymbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_spec" {
            if let Some(sym) = extract_type_spec(&child, source) {
                out.push(sym);
            }
        }
    }
}

fn extract_type_spec(node: &Node, source: &[u8]) -> Option<ExtractedSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source);
    let type_node = node.child_by_field_name("type")?;

    let kind = match type_node.kind() {
        "struct_type" => SymbolKind::Struct,
        "interface_type" => SymbolKind::Interface,
        _ => SymbolKind::TypeAlias,
    };

    // Use the parent type_declaration node for line range if available
    let decl_node = node.parent().unwrap_or(*node);
    let body = node_text(&decl_node, source);
    let signature = format!("type {name} {}", type_node.kind().replace('_', " "));

    Some(ExtractedSymbol {
        name,
        kind,
        signature,
        body,
        start_line: decl_node.start_position().row + 1,
        end_line: decl_node.end_position().row + 1,
        children: Vec::new(),
    })
}

fn extract_const_or_var(
    node: &Node,
    source: &[u8],
    kind: SymbolKind,
    spec_kind: &str,
    out: &mut Vec<ExtractedSymbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == spec_kind {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source);
                let body = node_text(node, source);
                let signature = extract_declaration_line(&body);
                out.push(ExtractedSymbol {
                    name,
                    kind: kind.clone(),
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
/// For `func HandleRequest(w http.ResponseWriter, r *http.Request) {`,
/// the signature is `func HandleRequest(w http.ResponseWriter, r *http.Request)`.
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
/// For single-line items (const, var), returns the full text.
fn extract_declaration_line(body: &str) -> String {
    match body.find('{') {
        Some(idx) => body[..idx].trim().to_string(),
        None => body.trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Reference extraction
// ---------------------------------------------------------------------------

/// Walk the entire tree collecting call references. For each call_expression,
/// figure out which enclosing function/method it lives in, and record
/// (from_symbol, to_name, "calls").
fn collect_references(
    root: &Node,
    source: &[u8],
    refs: &mut Vec<ExtractedReference>,
) {
    let mut stack: Vec<Node> = vec![*root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(callee) = extract_callee(&node, source) {
                let from = find_enclosing_function(&node, source)
                    .unwrap_or_default();
                refs.push(ExtractedReference {
                    from_symbol: from,
                    to_name: callee,
                    kind: "calls".to_string(),
                });
            }
        }
        // Push children in reverse order so we visit left-to-right
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

/// Extract the callee name from a call_expression node.
/// Handles `Println(...)` and `fmt.Println(...)` (strips the package prefix).
fn extract_callee(node: &Node, source: &[u8]) -> Option<String> {
    let func_node = node.child_by_field_name("function")?;
    match func_node.kind() {
        "identifier" => Some(node_text(&func_node, source)),
        "selector_expression" => {
            // e.g. fmt.Println — extract the field (right-hand side)
            let field = func_node.child_by_field_name("field")?;
            Some(node_text(&field, source))
        }
        _ => {
            // Fallback: grab the raw text
            Some(node_text(&func_node, source))
        }
    }
}

/// Walk up from `node` to find the nearest enclosing function_declaration or
/// method_declaration, and return its name.
fn find_enclosing_function(node: &Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        match n.kind() {
            "function_declaration" | "method_declaration" => {
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
