use std::fs;

use tempfile::TempDir;
use focal_core::db::Database;
use focal_core::grammar::GrammarRegistry;
use focal_core::indexer::Indexer;

/// Helper: create an in-memory DB + grammar registry, return (db, registry).
fn setup() -> (Database, GrammarRegistry) {
    let db = Database::open_in_memory().unwrap();
    let registry = GrammarRegistry::new();
    (db, registry)
}

/// Write a Go file into `dir` at the given relative path.
fn write_go_file(dir: &TempDir, rel_path: &str, content: &str) {
    let full = dir.path().join(rel_path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
}

const TWO_FUNC_GO: &str = r#"package main

func Alpha() {
    println("alpha")
}

func Beta() {
    println("beta")
}
"#;

// ---------------------------------------------------------------------------
// 1. Index Go files — verify stats and symbols in DB
// ---------------------------------------------------------------------------
#[test]
fn test_index_go_files() {
    let (db, registry) = setup();
    let dir = TempDir::new().unwrap();
    write_go_file(&dir, "main.go", TWO_FUNC_GO);

    let indexer = Indexer::new(&db, &registry);
    let stats = indexer.index_directory(dir.path()).unwrap();

    assert_eq!(stats.files_indexed, 1, "expected 1 file indexed");
    assert!(
        stats.symbols_extracted >= 2,
        "expected >= 2 symbols, got {}",
        stats.symbols_extracted
    );
    assert!(stats.errors.is_empty(), "unexpected errors: {:?}", stats.errors);

    // Verify symbols exist in DB
    let root = dir.path().canonicalize().unwrap();
    let root_str = root.to_string_lossy().to_string();
    let repo = db.get_repository_by_path(&root_str).unwrap().unwrap();
    let files = db.get_files_for_repo(repo.id).unwrap();
    assert_eq!(files.len(), 1);

    let symbols = db.get_symbols_by_file(files[0].id).unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Alpha"), "missing Alpha, got: {names:?}");
    assert!(names.contains(&"Beta"), "missing Beta, got: {names:?}");
}

// ---------------------------------------------------------------------------
// 2. Skip unchanged files on re-index
// ---------------------------------------------------------------------------
#[test]
fn test_skip_unchanged_files() {
    let (db, registry) = setup();
    let dir = TempDir::new().unwrap();
    write_go_file(&dir, "main.go", TWO_FUNC_GO);

    let indexer = Indexer::new(&db, &registry);

    // First index
    let stats1 = indexer.index_directory(dir.path()).unwrap();
    assert_eq!(stats1.files_indexed, 1);

    // Second index — same content, should skip
    let stats2 = indexer.index_directory(dir.path()).unwrap();
    assert_eq!(stats2.files_indexed, 0, "expected 0 re-indexed files");
    assert_eq!(stats2.files_skipped, 1, "expected 1 skipped file");
}

// ---------------------------------------------------------------------------
// 3. Edge resolution — function A calls function B
// ---------------------------------------------------------------------------
#[test]
fn test_edge_resolution() {
    let (db, registry) = setup();
    let dir = TempDir::new().unwrap();

    let go_source = r#"package main

func Caller() {
    Callee()
}

func Callee() {
    println("done")
}
"#;
    write_go_file(&dir, "main.go", go_source);

    let indexer = Indexer::new(&db, &registry);
    let stats = indexer.index_directory(dir.path()).unwrap();

    assert!(
        stats.edges_created >= 1,
        "expected >= 1 edge, got {}",
        stats.edges_created
    );

    // Find the Caller symbol and verify it has a dependency on Callee
    let root = dir.path().canonicalize().unwrap();
    let root_str = root.to_string_lossy().to_string();
    let repo = db.get_repository_by_path(&root_str).unwrap().unwrap();

    let caller = db.find_symbol_by_name(repo.id, "Caller").unwrap().unwrap();
    let deps = db.get_dependencies(caller.id).unwrap();
    let dep_names: Vec<&str> = deps.iter().map(|(_, s)| s.name.as_str()).collect();
    assert!(
        dep_names.contains(&"Callee"),
        "expected Caller -> Callee edge, got deps: {dep_names:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Exclude patterns — node_modules/ should be skipped
// ---------------------------------------------------------------------------
#[test]
fn test_exclude_patterns() {
    let (db, registry) = setup();
    let dir = TempDir::new().unwrap();

    // A normal Go file
    write_go_file(&dir, "main.go", TWO_FUNC_GO);

    // A Go file inside node_modules — should be excluded
    write_go_file(
        &dir,
        "node_modules/dep/dep.go",
        "package dep\n\nfunc Hidden() {}\n",
    );

    let indexer = Indexer::new(&db, &registry);
    let stats = indexer.index_directory(dir.path()).unwrap();

    // Only main.go should be indexed
    assert_eq!(stats.files_indexed, 1, "expected only 1 file indexed (node_modules excluded)");

    // Verify no symbol named Hidden exists
    let root = dir.path().canonicalize().unwrap();
    let root_str = root.to_string_lossy().to_string();
    let repo = db.get_repository_by_path(&root_str).unwrap().unwrap();
    let hidden = db.find_symbol_by_name(repo.id, "Hidden").unwrap();
    assert!(hidden.is_none(), "Hidden should not be indexed (it's in node_modules)");
}
