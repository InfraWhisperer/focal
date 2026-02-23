use std::path::Path;

use focal_core::grammar::go::GoGrammar;
use focal_core::grammar::{Grammar, GrammarRegistry, SymbolKind};

const GO_SOURCE: &str = r#"
package main

import "fmt"

type Server struct {
    Port int
    Host string
}

func (s *Server) Start() error {
    fmt.Println("starting")
    return nil
}

func HandleRequest(w http.ResponseWriter, r *http.Request) {
    w.Write([]byte("ok"))
}

const MaxRetries = 3
"#;

fn parse_go(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    parser.set_language(&lang).expect("failed to set Go language");
    parser.parse(source.as_bytes(), None).expect("failed to parse Go source")
}

// ---------------------------------------------------------------------------
// 1. Symbol extraction
// ---------------------------------------------------------------------------
#[test]
fn test_go_extract_symbols() {
    let tree = parse_go(GO_SOURCE);
    let grammar = GoGrammar;
    let symbols = grammar.extract_symbols(GO_SOURCE.as_bytes(), &tree);

    let names: Vec<(&str, &SymbolKind)> = symbols
        .iter()
        .map(|s| (s.name.as_str(), &s.kind))
        .collect();

    // Server struct
    assert!(
        names.iter().any(|(n, k)| *n == "Server" && **k == SymbolKind::Struct),
        "expected Server (Struct), got: {names:?}"
    );

    // Start method
    assert!(
        names.iter().any(|(n, k)| *n == "Start" && **k == SymbolKind::Method),
        "expected Start (Method), got: {names:?}"
    );

    // HandleRequest function
    assert!(
        names.iter().any(|(n, k)| *n == "HandleRequest" && **k == SymbolKind::Function),
        "expected HandleRequest (Function), got: {names:?}"
    );

    // MaxRetries const
    assert!(
        names.iter().any(|(n, k)| *n == "MaxRetries" && **k == SymbolKind::Const),
        "expected MaxRetries (Const), got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Reference extraction
// ---------------------------------------------------------------------------
#[test]
fn test_go_extract_references() {
    let tree = parse_go(GO_SOURCE);
    let grammar = GoGrammar;
    let refs = grammar.extract_references(GO_SOURCE.as_bytes(), &tree);

    // Start calls Println (via fmt.Println)
    assert!(
        refs.iter().any(|r| r.from_symbol == "Start" && r.to_name == "Println"),
        "expected Start -> Println call, got: {refs:?}",
    );

    // HandleRequest calls Write (via w.Write)
    assert!(
        refs.iter().any(|r| r.from_symbol == "HandleRequest" && r.to_name == "Write"),
        "expected HandleRequest -> Write call, got: {refs:?}",
    );
}

// ---------------------------------------------------------------------------
// 3. Registry lookup
// ---------------------------------------------------------------------------
#[test]
fn test_grammar_registry() {
    let registry = GrammarRegistry::new();

    // Go is registered
    assert!(
        registry.for_extension("go").is_some(),
        "expected for_extension(\"go\") to return Some"
    );

    // Rust is registered
    assert!(
        registry.for_extension("rs").is_some(),
        "expected for_extension(\"rs\") to return Some"
    );

    // detect_language for a .go file
    let lang = registry.detect_language(Path::new("cmd/server/main.go"));
    assert_eq!(lang, Some("go"));

    // detect_language for unknown extension
    let lang = registry.detect_language(Path::new("notes.txt"));
    assert!(lang.is_none());
}

// ---------------------------------------------------------------------------
// 4. Signature extraction
// ---------------------------------------------------------------------------
#[test]
fn test_go_signature_extraction() {
    let tree = parse_go(GO_SOURCE);
    let grammar = GoGrammar;
    let symbols = grammar.extract_symbols(GO_SOURCE.as_bytes(), &tree);

    let handle_req = symbols
        .iter()
        .find(|s| s.name == "HandleRequest")
        .expect("HandleRequest not found");

    // Signature should contain the parameter list
    assert!(
        handle_req.signature.contains("http.ResponseWriter"),
        "signature should contain http.ResponseWriter, got: {:?}",
        handle_req.signature
    );
    assert!(
        handle_req.signature.contains("*http.Request"),
        "signature should contain *http.Request, got: {:?}",
        handle_req.signature
    );

    // Signature should NOT contain the function body
    assert!(
        !handle_req.signature.contains("[]byte"),
        "signature should not contain body content, got: {:?}",
        handle_req.signature
    );
}
