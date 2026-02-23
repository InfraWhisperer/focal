use focal_core::db::Database;

// ---------------------------------------------------------------------------
// Helper: seed a repo with a file containing multiple symbols (with bodies)
// ---------------------------------------------------------------------------
fn setup_db_with_symbols() -> (Database, i64, i64) {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test-repo", "/tmp/test-repo").unwrap();
    let file_id = db
        .upsert_file(repo_id, "src/handler.rs", "rust", "aaa111")
        .unwrap();

    db.insert_symbol(
        file_id,
        "Config",
        "struct",
        "pub struct Config",
        "pub struct Config { pub port: u16, pub host: String }",
        "",
        1,
        4,
        None,
    )
    .unwrap();

    db.insert_symbol(
        file_id,
        "handle_request",
        "function",
        "pub fn handle_request(req: Request) -> Response",
        "pub fn handle_request(req: Request) -> Response { let cfg = Config::default(); todo!() }",
        "",
        6,
        20,
        None,
    )
    .unwrap();

    db.insert_symbol(
        file_id,
        "validate",
        "function",
        "fn validate(input: &str) -> Result<(), Error>",
        "fn validate(input: &str) -> Result<(), Error> { if input.is_empty() { return Err(Error::Empty); } Ok(()) }",
        "",
        22,
        30,
        None,
    )
    .unwrap();

    (db, repo_id, file_id)
}

// ---------------------------------------------------------------------------
// 1. get_skeleton returns SymbolSummary (no body field) for a file_id
// ---------------------------------------------------------------------------
#[test]
fn test_get_skeleton_returns_summaries() {
    let (db, _repo_id, file_id) = setup_db_with_symbols();

    let skeletons = db.get_skeleton(file_id, "standard").unwrap();
    assert_eq!(skeletons.len(), 3);

    // Ordered by start_line
    assert_eq!(skeletons[0].name, "Config");
    assert_eq!(skeletons[0].kind, "struct");
    assert_eq!(skeletons[0].signature, "pub struct Config");
    assert_eq!(skeletons[0].start_line, 1);
    assert_eq!(skeletons[0].end_line, 4);

    assert_eq!(skeletons[1].name, "handle_request");
    assert_eq!(skeletons[1].kind, "function");
    assert_eq!(
        skeletons[1].signature,
        "pub fn handle_request(req: Request) -> Response"
    );

    assert_eq!(skeletons[2].name, "validate");
    assert_eq!(skeletons[2].kind, "function");
    assert_eq!(skeletons[2].start_line, 22);
    assert_eq!(skeletons[2].end_line, 30);
}

// ---------------------------------------------------------------------------
// 2. get_skeleton on an empty file returns empty vec
// ---------------------------------------------------------------------------
#[test]
fn test_get_skeleton_empty_file() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "empty.rs", "rust", "e").unwrap();

    let skeletons = db.get_skeleton(file_id, "minimal").unwrap();
    assert!(skeletons.is_empty());
}

// ---------------------------------------------------------------------------
// 3. get_skeleton_by_path resolves file via suffix match
// ---------------------------------------------------------------------------
#[test]
fn test_get_skeleton_by_path() {
    let (db, _repo_id, _file_id) = setup_db_with_symbols();

    // Full relative path
    let skeletons = db
        .get_skeleton_by_path("src/handler.rs", None, "standard")
        .unwrap();
    assert_eq!(skeletons.len(), 3);

    // Suffix-only match
    let skeletons = db
        .get_skeleton_by_path("handler.rs", None, "standard")
        .unwrap();
    assert_eq!(skeletons.len(), 3);

    // Non-existent file returns empty vec, not an error
    let skeletons = db
        .get_skeleton_by_path("nonexistent.rs", None, "standard")
        .unwrap();
    assert!(skeletons.is_empty());
}

// ---------------------------------------------------------------------------
// 4. get_skeleton_by_path with repo_name filter
// ---------------------------------------------------------------------------
#[test]
fn test_get_skeleton_by_path_with_repo_filter() {
    let (db, _repo_id, _file_id) = setup_db_with_symbols();

    // Matching repo name
    let skeletons = db
        .get_skeleton_by_path("handler.rs", Some("test-repo"), "standard")
        .unwrap();
    assert_eq!(skeletons.len(), 3);

    // Wrong repo name
    let skeletons = db
        .get_skeleton_by_path("handler.rs", Some("other-repo"), "standard")
        .unwrap();
    assert!(skeletons.is_empty());
}

// ---------------------------------------------------------------------------
// 5. detail parameter is accepted but all levels return the same result (v1)
// ---------------------------------------------------------------------------
#[test]
fn test_get_skeleton_detail_levels_are_equivalent() {
    let (db, _repo_id, file_id) = setup_db_with_symbols();

    let minimal = db.get_skeleton(file_id, "minimal").unwrap();
    let standard = db.get_skeleton(file_id, "standard").unwrap();
    let verbose = db.get_skeleton(file_id, "verbose").unwrap();

    assert_eq!(minimal.len(), standard.len());
    assert_eq!(standard.len(), verbose.len());

    for i in 0..minimal.len() {
        assert_eq!(minimal[i].name, standard[i].name);
        assert_eq!(standard[i].name, verbose[i].name);
        assert_eq!(minimal[i].signature, standard[i].signature);
    }
}
