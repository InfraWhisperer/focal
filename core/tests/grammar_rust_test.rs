use focal_core::grammar::rust_lang::RustGrammar;
use focal_core::grammar::{Grammar, SymbolKind};

const RUST_SOURCE: &str = r#"use std::io;

struct Config {
    port: u16,
    host: String,
}

impl Config {
    fn new(port: u16) -> Self {
        Config { port, host: String::from("localhost") }
    }

    fn validate(&self) -> bool {
        self.port > 0
    }
}

fn start_server(config: &Config) {
    println!("Starting on port {}", config.port);
}

trait Handler {
    fn handle(&self, request: &str) -> String;
}

enum Status {
    Active,
    Inactive,
}

const MAX_CONNECTIONS: u32 = 100;

type Result<T> = std::result::Result<T, io::Error>;
"#;

fn parse_rust(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    parser
        .set_language(&lang)
        .expect("failed to set Rust language");
    parser
        .parse(source.as_bytes(), None)
        .expect("failed to parse Rust source")
}

// ---------------------------------------------------------------------------
// 1. Symbol extraction
// ---------------------------------------------------------------------------
#[test]
fn test_rust_extract_symbols() {
    let tree = parse_rust(RUST_SOURCE);
    let grammar = RustGrammar;
    let symbols = grammar.extract_symbols(RUST_SOURCE.as_bytes(), &tree);

    let names: Vec<(&str, &SymbolKind)> = symbols
        .iter()
        .map(|s| (s.name.as_str(), &s.kind))
        .collect();

    // Config struct
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Config" && **k == SymbolKind::Struct),
        "expected Config (Struct), got: {names:?}"
    );

    // Config::new method (from impl Config)
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Config::new" && **k == SymbolKind::Method),
        "expected Config::new (Method), got: {names:?}"
    );

    // Config::validate method (from impl Config)
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Config::validate" && **k == SymbolKind::Method),
        "expected Config::validate (Method), got: {names:?}"
    );

    // start_server function
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "start_server" && **k == SymbolKind::Function),
        "expected start_server (Function), got: {names:?}"
    );

    // Handler trait
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Handler" && **k == SymbolKind::Trait),
        "expected Handler (Trait), got: {names:?}"
    );

    // Status enum
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Status" && **k == SymbolKind::Enum),
        "expected Status (Enum), got: {names:?}"
    );

    // MAX_CONNECTIONS const
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "MAX_CONNECTIONS" && **k == SymbolKind::Const),
        "expected MAX_CONNECTIONS (Const), got: {names:?}"
    );

    // Result type alias
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "Result" && **k == SymbolKind::TypeAlias),
        "expected Result (TypeAlias), got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Reference extraction
// ---------------------------------------------------------------------------
#[test]
fn test_rust_extract_references() {
    let tree = parse_rust(RUST_SOURCE);
    let grammar = RustGrammar;
    let refs = grammar.extract_references(RUST_SOURCE.as_bytes(), &tree);

    // new calls String::from => callee = "from"
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "new" && r.to_name == "from" && r.kind == "calls"),
        "expected new -> from call, got: {refs:?}"
    );

    // start_server calls println! macro
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "start_server" && r.to_name == "println" && r.kind == "calls"),
        "expected start_server -> println call, got: {refs:?}"
    );

    // import reference for use std::io
    assert!(
        refs.iter()
            .any(|r| r.kind == "imports" && r.to_name.contains("std::io")),
        "expected import reference for std::io, got: {refs:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Signature extraction
// ---------------------------------------------------------------------------
#[test]
fn test_rust_signature_extraction() {
    let tree = parse_rust(RUST_SOURCE);
    let grammar = RustGrammar;
    let symbols = grammar.extract_symbols(RUST_SOURCE.as_bytes(), &tree);

    let start_server = symbols
        .iter()
        .find(|s| s.name == "start_server")
        .expect("start_server not found");

    // Signature should contain parameter type
    assert!(
        start_server.signature.contains("&Config"),
        "signature should contain &Config, got: {:?}",
        start_server.signature
    );

    // Signature should NOT contain body content (println)
    assert!(
        !start_server.signature.contains("println"),
        "signature should not contain body content, got: {:?}",
        start_server.signature
    );

    let new_method = symbols
        .iter()
        .find(|s| s.name == "Config::new")
        .expect("Config::new method not found");

    // Method signature should have parameters and return type
    assert!(
        new_method.signature.contains("port: u16"),
        "new signature should contain port: u16, got: {:?}",
        new_method.signature
    );
    assert!(
        new_method.signature.contains("-> Self"),
        "new signature should contain -> Self, got: {:?}",
        new_method.signature
    );
}

// ---------------------------------------------------------------------------
// 4. Registry integration
// ---------------------------------------------------------------------------
#[test]
fn test_rust_registry() {
    let registry = focal_core::grammar::GrammarRegistry::new();
    assert!(
        registry.for_extension("rs").is_some(),
        "expected for_extension(\"rs\") to return Some"
    );
}
