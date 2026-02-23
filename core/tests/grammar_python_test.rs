use focal_core::grammar::python::PythonGrammar;
use focal_core::grammar::{Grammar, SymbolKind};

const PY_SOURCE: &str = r#"import os
from pathlib import Path

class FileProcessor:
    def __init__(self, base_dir: str):
        self.base_dir = Path(base_dir)

    def process(self, filename: str) -> bool:
        path = self.base_dir / filename
        return path.exists()

    def list_files(self) -> list:
        return os.listdir(self.base_dir)

def read_config(path: str) -> dict:
    with open(path) as f:
        return json.load(f)

def main():
    processor = FileProcessor("/tmp")
    processor.process("test.txt")
"#;

fn parse_python(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    parser
        .set_language(&lang)
        .expect("failed to set Python language");
    parser
        .parse(source.as_bytes(), None)
        .expect("failed to parse Python source")
}

// ---------------------------------------------------------------------------
// 1. Symbol extraction
// ---------------------------------------------------------------------------
#[test]
fn test_python_extract_symbols() {
    let tree = parse_python(PY_SOURCE);
    let grammar = PythonGrammar;
    let symbols = grammar.extract_symbols(PY_SOURCE.as_bytes(), &tree);

    let names: Vec<(&str, &SymbolKind)> = symbols
        .iter()
        .map(|s| (s.name.as_str(), &s.kind))
        .collect();

    // FileProcessor class
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "FileProcessor" && **k == SymbolKind::Class),
        "expected FileProcessor (Class), got: {names:?}"
    );

    // read_config function
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "read_config" && **k == SymbolKind::Function),
        "expected read_config (Function), got: {names:?}"
    );

    // main function
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "main" && **k == SymbolKind::Function),
        "expected main (Function), got: {names:?}"
    );

    // Class should have method children
    let file_processor = symbols
        .iter()
        .find(|s| s.name == "FileProcessor")
        .expect("FileProcessor not found");
    let method_names: Vec<&str> = file_processor
        .children
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert!(
        method_names.contains(&"__init__"),
        "expected __init__ method, got: {method_names:?}"
    );
    assert!(
        method_names.contains(&"process"),
        "expected process method, got: {method_names:?}"
    );
    assert!(
        method_names.contains(&"list_files"),
        "expected list_files method, got: {method_names:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Reference extraction
// ---------------------------------------------------------------------------
#[test]
fn test_python_extract_references() {
    let tree = parse_python(PY_SOURCE);
    let grammar = PythonGrammar;
    let refs = grammar.extract_references(PY_SOURCE.as_bytes(), &tree);

    // main calls FileProcessor
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "main" && r.to_name == "FileProcessor"),
        "expected main -> FileProcessor call, got: {refs:?}"
    );

    // main calls process (via processor.process)
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "main" && r.to_name == "process"),
        "expected main -> process call, got: {refs:?}"
    );

    // read_config calls open
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "read_config" && r.to_name == "open"),
        "expected read_config -> open call, got: {refs:?}"
    );

    // read_config calls json.load => callee = "load"
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "read_config" && r.to_name == "load"),
        "expected read_config -> load call, got: {refs:?}"
    );

    // import references
    assert!(
        refs.iter()
            .any(|r| r.kind == "imports" && r.to_name.contains("import os")),
        "expected import reference for os, got: {refs:?}"
    );
    assert!(
        refs.iter()
            .any(|r| r.kind == "imports" && r.to_name.contains("from pathlib")),
        "expected import reference for pathlib, got: {refs:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Signature extraction
// ---------------------------------------------------------------------------
#[test]
fn test_python_signature_extraction() {
    let tree = parse_python(PY_SOURCE);
    let grammar = PythonGrammar;
    let symbols = grammar.extract_symbols(PY_SOURCE.as_bytes(), &tree);

    let read_config = symbols
        .iter()
        .find(|s| s.name == "read_config")
        .expect("read_config not found");

    // Signature should contain parameter and return type
    assert!(
        read_config.signature.contains("path: str"),
        "signature should contain path: str, got: {:?}",
        read_config.signature
    );
    assert!(
        read_config.signature.contains("-> dict"),
        "signature should contain -> dict, got: {:?}",
        read_config.signature
    );

    // Signature should NOT contain body content
    assert!(
        !read_config.signature.contains("json.load"),
        "signature should not contain body content, got: {:?}",
        read_config.signature
    );
}

// ---------------------------------------------------------------------------
// 4. Registry integration
// ---------------------------------------------------------------------------
#[test]
fn test_python_registry() {
    let registry = focal_core::grammar::GrammarRegistry::new();
    assert!(
        registry.for_extension("py").is_some(),
        "expected for_extension(\"py\") to return Some"
    );
    assert!(
        registry.for_extension("pyi").is_some(),
        "expected for_extension(\"pyi\") to return Some"
    );
}
