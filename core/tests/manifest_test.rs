use focal_core::db::Database;
use focal_core::graph::GraphEngine;
use focal_core::manifest::{
    export_manifest, import_manifest, Manifest, ManifestEdge, ManifestSymbol, MANIFEST_VERSION,
};

/// Helper: create an in-memory DB with a repo, a file, and return (db, repo_id, file_id).
fn setup_db() -> (Database, i64, i64) {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test-repo", "/tmp/test-repo").unwrap();
    let file_id = db
        .upsert_file(repo_id, "src/lib.rs", "rust", "abc123")
        .unwrap();
    (db, repo_id, file_id)
}

/// Build a minimal manifest struct for testing import paths.
fn make_manifest(
    repo: &str,
    symbols: Vec<ManifestSymbol>,
    edges: Vec<ManifestEdge>,
) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        repo: repo.to_string(),
        exported_at: "1700000000".to_string(),
        focal_version: "0.1.0-test".to_string(),
        symbols,
        edges,
    }
}

fn make_symbol(name: &str, qualified_name: &str, signature: &str) -> ManifestSymbol {
    ManifestSymbol {
        name: name.to_string(),
        qualified_name: qualified_name.to_string(),
        kind: "function".to_string(),
        file: "src/service.rs".to_string(),
        line_start: 1,
        line_end: 10,
        signature: signature.to_string(),
        language: "rust".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 1. Export produces valid manifest structure
// ---------------------------------------------------------------------------
#[test]
fn test_export_manifest_structure() {
    let (db, repo_id, file_id) = setup_db();

    db.insert_symbol(
        file_id, "Alpha", "pkg.Alpha", "function", "fn Alpha()", "body_a", "h1", 1, 10, None,
    )
    .unwrap();
    db.insert_symbol(
        file_id, "Beta", "pkg.Beta", "function", "fn Beta(x: i32)", "body_b", "h2", 11, 20, None,
    )
    .unwrap();

    let manifest = export_manifest(&db, repo_id, "test-repo").unwrap();

    assert_eq!(manifest.version, MANIFEST_VERSION);
    assert_eq!(manifest.repo, "test-repo");
    assert!(!manifest.exported_at.is_empty());
    assert_eq!(manifest.symbols.len(), 2);

    // ManifestSymbol has no body field — the type system enforces this at compile
    // time, but verify signatures propagated correctly.
    let alpha = manifest.symbols.iter().find(|s| s.name == "Alpha").unwrap();
    assert_eq!(alpha.signature, "fn Alpha()");
    assert_eq!(alpha.qualified_name, "pkg.Alpha");

    let beta = manifest.symbols.iter().find(|s| s.name == "Beta").unwrap();
    assert_eq!(beta.signature, "fn Beta(x: i32)");
}

// ---------------------------------------------------------------------------
// 2. Import creates symbols with correct source/manifest_repo
// ---------------------------------------------------------------------------
#[test]
fn test_import_manifest_symbols() {
    let db = Database::open_in_memory().unwrap();

    let manifest = make_manifest(
        "remote-svc",
        vec![
            make_symbol("Foo", "svc.Foo", "fn Foo()"),
            make_symbol("Bar", "svc.Bar", "fn Bar(x: u64)"),
        ],
        vec![],
    );

    let (sym_count, edge_count) = import_manifest(&db, &manifest).unwrap();
    assert_eq!(sym_count, 2);
    assert_eq!(edge_count, 0);

    let foo = db.find_symbol_by_name_any("Foo").unwrap().unwrap();
    assert_eq!(foo.source, "manifest");
    assert_eq!(foo.manifest_repo.as_deref(), Some("remote-svc"));
    assert_eq!(foo.body, "", "manifest symbols carry no body");
}

// ---------------------------------------------------------------------------
// 3. Reimport replaces, doesn't duplicate
// ---------------------------------------------------------------------------
#[test]
fn test_reimport_replaces_symbols() {
    let db = Database::open_in_memory().unwrap();

    let manifest = make_manifest(
        "dup-repo",
        vec![
            make_symbol("X", "dup.X", "fn X()"),
            make_symbol("Y", "dup.Y", "fn Y()"),
        ],
        vec![],
    );

    let (c1, _) = import_manifest(&db, &manifest).unwrap();
    assert_eq!(c1, 2);

    let (c2, _) = import_manifest(&db, &manifest).unwrap();
    assert_eq!(c2, 2);

    let count = db.count_symbols_for_manifest("dup-repo").unwrap();
    assert_eq!(count, 2, "reimport should replace, not duplicate — got {count}");
}

// ---------------------------------------------------------------------------
// 4. Cross-repo graph traversal via manifest edges
// ---------------------------------------------------------------------------
#[test]
fn test_cross_repo_graph_traversal() {
    let (db, _repo_id, file_id) = setup_db();

    // Local symbol in repo A
    let handle_id = db
        .insert_symbol(
            file_id, "handle_request", "a.handle_request", "function",
            "fn handle_request()", "", "", 1, 10, None,
        )
        .unwrap();

    // Import manifest from repo B with two symbols and an internal edge
    let manifest = make_manifest(
        "repo-b",
        vec![
            make_symbol("ProcessPayment", "b.ProcessPayment", "fn ProcessPayment()"),
            make_symbol("ExecuteTx", "b.ExecuteTx", "fn ExecuteTx()"),
        ],
        vec![ManifestEdge {
            source: "b.ProcessPayment".to_string(),
            target: "b.ExecuteTx".to_string(),
            kind: "calls".to_string(),
        }],
    );

    let (sym_count, edge_count) = import_manifest(&db, &manifest).unwrap();
    assert_eq!(sym_count, 2);
    assert_eq!(edge_count, 1);

    // Find manifest symbol ID for ProcessPayment
    let pp = db.find_symbol_by_name_any("ProcessPayment").unwrap().unwrap();

    // Cross-repo edge: handle_request -> ProcessPayment
    db.insert_edge(handle_id, pp.id, "calls").unwrap();

    // get_dependents(ProcessPayment) should include handle_request
    let dependents = db.get_dependents(pp.id).unwrap();
    let dep_names: Vec<&str> = dependents.iter().map(|(_, s)| s.name.as_str()).collect();
    assert!(
        dep_names.contains(&"handle_request"),
        "handle_request should be a dependent of ProcessPayment, got: {dep_names:?}"
    );

    // GraphEngine::impact_graph on ProcessPayment — should surface handle_request
    let engine = GraphEngine::new(&db);
    let impact = engine.impact_graph("ProcessPayment", 2, None).unwrap();
    let impact_names: Vec<&str> = impact.iter().map(|n| n.name.as_str()).collect();
    assert!(
        impact_names.contains(&"handle_request"),
        "impact graph should include handle_request, got: {impact_names:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Local takes precedence on collision
// ---------------------------------------------------------------------------
#[test]
fn test_local_takes_precedence_on_collision() {
    let (db, _repo_id, file_id) = setup_db();

    // Local symbol with qualified_name "utils.Helper"
    db.insert_symbol(
        file_id, "Helper", "utils.Helper", "function", "fn Helper()", "local body", "lh", 1, 5, None,
    )
    .unwrap();

    // Manifest with same qualified_name — should be skipped
    let manifest = make_manifest(
        "collider",
        vec![make_symbol("Helper", "utils.Helper", "fn Helper(remote: bool)")],
        vec![],
    );

    let (sym_count, _) = import_manifest(&db, &manifest).unwrap();
    assert_eq!(sym_count, 0, "manifest symbol should be skipped on collision with local");

    // The symbol returned should be the local one
    let helper = db.find_symbol_by_name_any("Helper").unwrap().unwrap();
    assert_eq!(helper.source, "local", "local symbol should win over manifest");
    assert_eq!(helper.signature, "fn Helper()");
}

// ---------------------------------------------------------------------------
// 6. Export/import round-trip
// ---------------------------------------------------------------------------
#[test]
fn test_export_import_round_trip() {
    // --- Source DB: create symbols and edges, then export ---
    let (src_db, src_repo_id, src_file_id) = setup_db();

    let s1 = src_db
        .insert_symbol(
            src_file_id, "Init", "app.Init", "function", "fn Init()", "body1", "bh1", 1, 10, None,
        )
        .unwrap();
    let s2 = src_db
        .insert_symbol(
            src_file_id, "Run", "app.Run", "function", "fn Run()", "body2", "bh2", 11, 20, None,
        )
        .unwrap();
    let s3 = src_db
        .insert_symbol(
            src_file_id, "Shutdown", "app.Shutdown", "function", "fn Shutdown()", "body3", "bh3", 21, 30, None,
        )
        .unwrap();

    src_db.insert_edge(s1, s2, "calls").unwrap();
    src_db.insert_edge(s2, s3, "calls").unwrap();

    let manifest = export_manifest(&src_db, src_repo_id, "test-repo").unwrap();
    assert_eq!(manifest.symbols.len(), 3);
    assert_eq!(manifest.edges.len(), 2);

    // --- Destination DB: import the manifest ---
    let dst_db = Database::open_in_memory().unwrap();
    let (sym_count, edge_count) = import_manifest(&dst_db, &manifest).unwrap();

    assert_eq!(sym_count, 3);
    assert_eq!(edge_count, 2);

    // All symbols should have source="manifest" in the destination
    for name in &["Init", "Run", "Shutdown"] {
        let sym = dst_db.find_symbol_by_name_any(name).unwrap().unwrap();
        assert_eq!(sym.source, "manifest", "{name} should be a manifest symbol");
        assert_eq!(sym.manifest_repo.as_deref(), Some("test-repo"));
        assert_eq!(sym.body, "", "manifest symbols carry no body");
    }

    // Verify edges survived the round trip by checking dependencies
    let init = dst_db.find_symbol_by_name_any("Init").unwrap().unwrap();
    let deps = dst_db.get_dependencies(init.id).unwrap();
    let dep_names: Vec<&str> = deps.iter().map(|(_, s)| s.name.as_str()).collect();
    assert!(dep_names.contains(&"Run"), "Init should depend on Run, got: {dep_names:?}");
}
