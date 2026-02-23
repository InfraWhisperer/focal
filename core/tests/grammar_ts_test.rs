use focal_core::grammar::typescript::TypeScriptGrammar;
use focal_core::grammar::{Grammar, SymbolKind};

const TS_SOURCE: &str = r#"import { Request, Response } from 'express';

interface UserConfig {
    name: string;
    email: string;
}

class UserService {
    private users: Map<string, UserConfig>;

    constructor() {
        this.users = new Map();
    }

    getUser(id: string): UserConfig | undefined {
        return this.users.get(id);
    }

    createUser(config: UserConfig): void {
        this.users.set(config.name, config);
    }
}

function handleRequest(req: Request, res: Response): void {
    const service = new UserService();
    const user = service.getUser(req.params.id);
    res.json(user);
}

type UserId = string;

const MAX_USERS = 1000;
"#;

fn parse_ts(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    parser
        .set_language(&lang)
        .expect("failed to set TypeScript language");
    parser
        .parse(source.as_bytes(), None)
        .expect("failed to parse TypeScript source")
}

// ---------------------------------------------------------------------------
// 1. Symbol extraction
// ---------------------------------------------------------------------------
#[test]
fn test_ts_extract_symbols() {
    let tree = parse_ts(TS_SOURCE);
    let grammar = TypeScriptGrammar;
    let symbols = grammar.extract_symbols(TS_SOURCE.as_bytes(), &tree);

    let names: Vec<(&str, &SymbolKind)> = symbols
        .iter()
        .map(|s| (s.name.as_str(), &s.kind))
        .collect();

    // UserConfig interface
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "UserConfig" && **k == SymbolKind::Interface),
        "expected UserConfig (Interface), got: {names:?}"
    );

    // UserService class
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "UserService" && **k == SymbolKind::Class),
        "expected UserService (Class), got: {names:?}"
    );

    // handleRequest function
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "handleRequest" && **k == SymbolKind::Function),
        "expected handleRequest (Function), got: {names:?}"
    );

    // UserId type alias
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "UserId" && **k == SymbolKind::TypeAlias),
        "expected UserId (TypeAlias), got: {names:?}"
    );

    // MAX_USERS const
    assert!(
        names
            .iter()
            .any(|(n, k)| *n == "MAX_USERS" && **k == SymbolKind::Const),
        "expected MAX_USERS (Const), got: {names:?}"
    );

    // Class should have method children
    let user_service = symbols
        .iter()
        .find(|s| s.name == "UserService")
        .expect("UserService not found");
    let method_names: Vec<&str> = user_service
        .children
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert!(
        method_names.contains(&"constructor"),
        "expected constructor method in UserService children, got: {method_names:?}"
    );
    assert!(
        method_names.contains(&"getUser"),
        "expected getUser method in UserService children, got: {method_names:?}"
    );
    assert!(
        method_names.contains(&"createUser"),
        "expected createUser method in UserService children, got: {method_names:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Reference extraction
// ---------------------------------------------------------------------------
#[test]
fn test_ts_extract_references() {
    let tree = parse_ts(TS_SOURCE);
    let grammar = TypeScriptGrammar;
    let refs = grammar.extract_references(TS_SOURCE.as_bytes(), &tree);

    // handleRequest calls getUser (via service.getUser)
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "handleRequest" && r.to_name == "getUser"),
        "expected handleRequest -> getUser call, got: {refs:?}"
    );

    // handleRequest calls json (via res.json)
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "handleRequest" && r.to_name == "json"),
        "expected handleRequest -> json call, got: {refs:?}"
    );

    // constructor calls new Map() => callee = "Map"
    assert!(
        refs.iter()
            .any(|r| r.from_symbol == "constructor" && r.to_name == "Map"),
        "expected constructor -> Map new call, got: {refs:?}"
    );

    // import reference
    assert!(
        refs.iter()
            .any(|r| r.kind == "imports" && r.to_name.contains("express")),
        "expected import reference for express, got: {refs:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Signature extraction
// ---------------------------------------------------------------------------
#[test]
fn test_ts_signature_extraction() {
    let tree = parse_ts(TS_SOURCE);
    let grammar = TypeScriptGrammar;
    let symbols = grammar.extract_symbols(TS_SOURCE.as_bytes(), &tree);

    let handle_req = symbols
        .iter()
        .find(|s| s.name == "handleRequest")
        .expect("handleRequest not found");

    // Signature should contain parameter types
    assert!(
        handle_req.signature.contains("Request"),
        "signature should contain Request, got: {:?}",
        handle_req.signature
    );
    assert!(
        handle_req.signature.contains("Response"),
        "signature should contain Response, got: {:?}",
        handle_req.signature
    );

    // Signature should NOT contain body content
    assert!(
        !handle_req.signature.contains("new UserService"),
        "signature should not contain body content, got: {:?}",
        handle_req.signature
    );
}

// ---------------------------------------------------------------------------
// 4. Registry integration
// ---------------------------------------------------------------------------
#[test]
fn test_ts_registry() {
    let registry = focal_core::grammar::GrammarRegistry::new();
    assert!(
        registry.for_extension("ts").is_some(),
        "expected for_extension(\"ts\") to return Some"
    );
    assert!(
        registry.for_extension("tsx").is_some(),
        "expected for_extension(\"tsx\") to return Some"
    );
    assert!(
        registry.for_extension("js").is_some(),
        "expected for_extension(\"js\") to return Some"
    );
}
