use focal_core::db::Database;
use focal_core::graph::GraphEngine;

/// Helper: create an in-memory DB with a repo, a file, and return (db, repo_id, file_id).
fn setup_db() -> (Database, i64, i64) {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test-repo", "/tmp/test-repo").unwrap();
    let file_id = db
        .upsert_file(repo_id, "src/lib.rs", "rust", "abc123")
        .unwrap();
    (db, repo_id, file_id)
}

// ---------------------------------------------------------------------------
// 1. Impact graph: A <- B <- C <- D chain
//    Changing A should show B, C, D as affected (with depth 3).
// ---------------------------------------------------------------------------
#[test]
fn test_impact_graph_linear_chain() {
    let (db, repo_id, file_id) = setup_db();

    // Create symbols: A, B, C, D
    let a = db
        .insert_symbol(file_id, "A", "function", "fn A()", "", 1, 5, None)
        .unwrap();
    let b = db
        .insert_symbol(file_id, "B", "function", "fn B()", "", 6, 10, None)
        .unwrap();
    let c = db
        .insert_symbol(file_id, "C", "function", "fn C()", "", 11, 15, None)
        .unwrap();
    let d = db
        .insert_symbol(file_id, "D", "function", "fn D()", "", 16, 20, None)
        .unwrap();

    // Edges: B -> A (B calls/depends on A), C -> B, D -> C
    // This means A is depended on by B, B by C, C by D.
    db.insert_edge(b, a, "calls").unwrap();
    db.insert_edge(c, b, "calls").unwrap();
    db.insert_edge(d, c, "calls").unwrap();

    let engine = GraphEngine::new(&db);

    // Impact of changing A with depth 3: should find B (dist 1), C (dist 2), D (dist 3)
    let nodes = engine.impact_graph("A", 3, Some(repo_id)).unwrap();
    assert_eq!(nodes.len(), 3, "expected 3 impacted nodes, got {:?}", nodes);

    let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"B"), "B should be affected");
    assert!(names.contains(&"C"), "C should be affected");
    assert!(names.contains(&"D"), "D should be affected");

    // Verify distances
    let b_node = nodes.iter().find(|n| n.name == "B").unwrap();
    assert_eq!(b_node.distance, 1);
    let c_node = nodes.iter().find(|n| n.name == "C").unwrap();
    assert_eq!(c_node.distance, 2);
    let d_node = nodes.iter().find(|n| n.name == "D").unwrap();
    assert_eq!(d_node.distance, 3);

    // With depth 1, only B is visible
    let nodes = engine.impact_graph("A", 1, Some(repo_id)).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "B");
    assert_eq!(nodes[0].distance, 1);
    assert_eq!(nodes[0].edge_kind, "calls");
}

// ---------------------------------------------------------------------------
// 2. Impact graph: handles cycles without infinite loop
// ---------------------------------------------------------------------------
#[test]
fn test_impact_graph_cycle() {
    let (db, repo_id, file_id) = setup_db();

    let x = db
        .insert_symbol(file_id, "X", "function", "fn X()", "", 1, 5, None)
        .unwrap();
    let y = db
        .insert_symbol(file_id, "Y", "function", "fn Y()", "", 6, 10, None)
        .unwrap();

    // Mutual dependency: X -> Y and Y -> X
    db.insert_edge(x, y, "calls").unwrap();
    db.insert_edge(y, x, "calls").unwrap();

    let engine = GraphEngine::new(&db);
    let nodes = engine.impact_graph("X", 5, Some(repo_id)).unwrap();

    // Y depends on X (via the X->Y edge, Y is in get_dependents(X)? No:
    // get_dependents(X) returns symbols where X is the target, i.e. edges where target=X.
    // Edge Y->X has target=X, so Y is a dependent of X. Correct.
    // Then get_dependents(Y) returns symbols where Y is the target. Edge X->Y has target=Y,
    // so X is a dependent of Y. But X is already visited, so no infinite loop.
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "Y");
}

// ---------------------------------------------------------------------------
// 3. Impact graph: unknown symbol returns error
// ---------------------------------------------------------------------------
#[test]
fn test_impact_graph_unknown_symbol() {
    let (db, _repo_id, _file_id) = setup_db();
    let engine = GraphEngine::new(&db);
    let result = engine.impact_graph("NonExistent", 2, None);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

// ---------------------------------------------------------------------------
// 4. Logic flow: main -> HandleRequest -> Process -> SaveToDB
// ---------------------------------------------------------------------------
#[test]
fn test_logic_flow_linear_path() {
    let (db, repo_id, file_id) = setup_db();

    let main_sym = db
        .insert_symbol(file_id, "main", "function", "fn main()", "", 1, 5, None)
        .unwrap();
    let handle = db
        .insert_symbol(file_id, "HandleRequest", "function", "fn HandleRequest()", "", 6, 10, None)
        .unwrap();
    let process = db
        .insert_symbol(file_id, "Process", "function", "fn Process()", "", 11, 15, None)
        .unwrap();
    let save = db
        .insert_symbol(file_id, "SaveToDB", "function", "fn SaveToDB()", "", 16, 20, None)
        .unwrap();

    // Forward edges: main -> HandleRequest -> Process -> SaveToDB
    db.insert_edge(main_sym, handle, "calls").unwrap();
    db.insert_edge(handle, process, "calls").unwrap();
    db.insert_edge(process, save, "calls").unwrap();

    let engine = GraphEngine::new(&db);
    let paths = engine
        .find_paths("main", "SaveToDB", 3, Some(repo_id))
        .unwrap();

    assert_eq!(paths.len(), 1, "expected exactly 1 path");
    let names: Vec<&str> = paths[0].iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["main", "HandleRequest", "Process", "SaveToDB"]);
}

// ---------------------------------------------------------------------------
// 5. Logic flow: multiple paths
// ---------------------------------------------------------------------------
#[test]
fn test_logic_flow_multiple_paths() {
    let (db, repo_id, file_id) = setup_db();

    let start = db
        .insert_symbol(file_id, "Start", "function", "fn Start()", "", 1, 5, None)
        .unwrap();
    let mid_a = db
        .insert_symbol(file_id, "MidA", "function", "fn MidA()", "", 6, 10, None)
        .unwrap();
    let mid_b = db
        .insert_symbol(file_id, "MidB", "function", "fn MidB()", "", 11, 15, None)
        .unwrap();
    let end = db
        .insert_symbol(file_id, "End", "function", "fn End()", "", 16, 20, None)
        .unwrap();

    // Two paths: Start -> MidA -> End, Start -> MidB -> End
    db.insert_edge(start, mid_a, "calls").unwrap();
    db.insert_edge(start, mid_b, "calls").unwrap();
    db.insert_edge(mid_a, end, "calls").unwrap();
    db.insert_edge(mid_b, end, "calls").unwrap();

    let engine = GraphEngine::new(&db);
    let paths = engine
        .find_paths("Start", "End", 5, Some(repo_id))
        .unwrap();

    assert_eq!(paths.len(), 2, "expected 2 paths, got {}", paths.len());

    let path_names: Vec<Vec<String>> = paths
        .iter()
        .map(|p| p.iter().map(|s| s.name.clone()).collect())
        .collect();

    // Both paths should start with Start and end with End
    for p in &path_names {
        assert_eq!(p.first().unwrap(), "Start");
        assert_eq!(p.last().unwrap(), "End");
        assert_eq!(p.len(), 3); // Start -> Mid -> End
    }
}

// ---------------------------------------------------------------------------
// 6. Logic flow: no path exists
// ---------------------------------------------------------------------------
#[test]
fn test_logic_flow_no_path() {
    let (db, repo_id, file_id) = setup_db();

    db.insert_symbol(file_id, "Isolated1", "function", "fn Isolated1()", "", 1, 5, None)
        .unwrap();
    db.insert_symbol(file_id, "Isolated2", "function", "fn Isolated2()", "", 6, 10, None)
        .unwrap();
    // No edges between them

    let engine = GraphEngine::new(&db);
    let paths = engine
        .find_paths("Isolated1", "Isolated2", 3, Some(repo_id))
        .unwrap();

    assert!(paths.is_empty(), "expected no paths between disconnected symbols");
}

// ---------------------------------------------------------------------------
// 7. Logic flow: source == target returns single-element path
// ---------------------------------------------------------------------------
#[test]
fn test_logic_flow_same_symbol() {
    let (db, repo_id, file_id) = setup_db();

    db.insert_symbol(file_id, "SelfRef", "function", "fn SelfRef()", "", 1, 5, None)
        .unwrap();

    let engine = GraphEngine::new(&db);
    let paths = engine
        .find_paths("SelfRef", "SelfRef", 3, Some(repo_id))
        .unwrap();

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].len(), 1);
    assert_eq!(paths[0][0].name, "SelfRef");
}
