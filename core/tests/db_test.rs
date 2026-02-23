use focal_core::db::Database;

// ---------------------------------------------------------------------------
// 1. Schema migration — all tables exist after open
// ---------------------------------------------------------------------------
#[test]
fn test_create_database_and_migrate() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();

    let expected = [
        "edges",
        "files",
        "memories",
        "memories_fts",
        "memory_symbols",
        "repositories",
        "symbols",
        "symbols_fts",
    ];
    for t in &expected {
        assert!(
            tables.iter().any(|name| name == t),
            "missing table: {t}, got: {tables:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Repository upsert + get
// ---------------------------------------------------------------------------
#[test]
fn test_upsert_and_get_repository() {
    let db = Database::open_in_memory().unwrap();

    let id1 = db.upsert_repository("myrepo", "/tmp/myrepo").unwrap();
    assert!(id1 > 0);

    let repo = db.get_repository_by_path("/tmp/myrepo").unwrap().unwrap();
    assert_eq!(repo.name, "myrepo");
    assert_eq!(repo.root_path, "/tmp/myrepo");

    // Upsert same path returns same id
    let id2 = db.upsert_repository("myrepo-renamed", "/tmp/myrepo").unwrap();
    assert_eq!(id1, id2);

    // Name should be updated
    let repo = db.get_repository_by_path("/tmp/myrepo").unwrap().unwrap();
    assert_eq!(repo.name, "myrepo-renamed");

    // get_repo_id_by_name
    let found = db.get_repo_id_by_name("myrepo-renamed").unwrap();
    assert_eq!(found, Some(id1));
    let missing = db.get_repo_id_by_name("nonexistent").unwrap();
    assert!(missing.is_none());
}

// ---------------------------------------------------------------------------
// 3. File + symbol CRUD
// ---------------------------------------------------------------------------
#[test]
fn test_file_and_symbol_crud() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();

    let file_id = db
        .upsert_file(repo_id, "src/main.rs", "rust", "abc123")
        .unwrap();
    assert!(file_id > 0);

    // get_file_by_path
    let f = db.get_file_by_path(repo_id, "src/main.rs").unwrap().unwrap();
    assert_eq!(f.language, "rust");
    assert_eq!(f.hash, "abc123");

    // get_file_hash
    let h = db.get_file_hash(repo_id, "src/main.rs").unwrap().unwrap();
    assert_eq!(h, "abc123");

    // upsert again with new hash
    let file_id2 = db
        .upsert_file(repo_id, "src/main.rs", "rust", "def456")
        .unwrap();
    assert_eq!(file_id, file_id2);
    let h = db.get_file_hash(repo_id, "src/main.rs").unwrap().unwrap();
    assert_eq!(h, "def456");

    // get_files_for_repo
    db.upsert_file(repo_id, "src/lib.rs", "rust", "ghi789")
        .unwrap();
    let files = db.get_files_for_repo(repo_id).unwrap();
    assert_eq!(files.len(), 2);

    // Insert symbol
    let sym_id = db
        .insert_symbol(
            file_id,
            "main",
            "function",
            "fn main()",
            "fn main() { println!(\"hello\"); }",
            1,
            3,
            None,
        )
        .unwrap();
    assert!(sym_id > 0);

    // get_symbols_by_file
    let syms = db.get_symbols_by_file(file_id).unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].name, "main");
    assert_eq!(syms[0].kind, "function");
    assert_eq!(syms[0].start_line, 1);
    assert_eq!(syms[0].end_line, 3);
    assert!(syms[0].parent_id.is_none());

    // find_symbol_by_name
    let found = db.find_symbol_by_name(repo_id, "main").unwrap().unwrap();
    assert_eq!(found.id, sym_id);

    // find_symbol_by_name_any
    let found = db.find_symbol_by_name_any("main").unwrap().unwrap();
    assert_eq!(found.id, sym_id);

    // get_file_path_for_symbol
    let path = db.get_file_path_for_symbol(sym_id).unwrap();
    assert_eq!(path, "src/main.rs");

    // delete_symbols_by_file
    let deleted = db.delete_symbols_by_file(file_id).unwrap();
    assert_eq!(deleted, 1);
    let syms = db.get_symbols_by_file(file_id).unwrap();
    assert!(syms.is_empty());
}

// ---------------------------------------------------------------------------
// 4. Edge CRUD
// ---------------------------------------------------------------------------
#[test]
fn test_edge_crud() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "lib.rs", "rust", "h1").unwrap();

    let s1 = db
        .insert_symbol(file_id, "foo", "function", "fn foo()", "", 1, 5, None)
        .unwrap();
    let s2 = db
        .insert_symbol(file_id, "bar", "function", "fn bar()", "", 6, 10, None)
        .unwrap();

    let edge_id = db.insert_edge(s1, s2, "calls").unwrap();
    assert!(edge_id > 0);

    // get_dependencies(s1) => [(edge, bar)]
    let deps = db.get_dependencies(s1).unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].1.name, "bar");
    assert_eq!(deps[0].0.kind, "calls");

    // get_dependents(s2) => [(edge, foo)]
    let dependents = db.get_dependents(s2).unwrap();
    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0].1.name, "foo");

    // delete_edges_by_file
    let deleted = db.delete_edges_by_file(file_id).unwrap();
    assert_eq!(deleted, 1);
    assert!(db.get_dependencies(s1).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 5. Memory CRUD
// ---------------------------------------------------------------------------
#[test]
fn test_memory_crud() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "", "", 1, 1, None)
        .unwrap();

    // Save memory linked to a symbol
    let mem_id = db
        .save_memory("this function is performance-critical", "note", &[sym_id])
        .unwrap();
    assert!(mem_id > 0);

    // list_memories — non-stale, no filter
    let mems = db.list_memories("", false, "").unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].content, "this function is performance-critical");
    assert!(!mems[0].stale);

    // list by category
    let mems = db.list_memories("note", false, "").unwrap();
    assert_eq!(mems.len(), 1);
    let mems = db.list_memories("bug", false, "").unwrap();
    assert!(mems.is_empty());

    // update_memory
    db.update_memory(mem_id, "updated content", "bug", &[sym_id])
        .unwrap();
    let mems = db.list_memories("bug", false, "").unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].content, "updated content");

    // delete_memory
    let deleted = db.delete_memory(mem_id).unwrap();
    assert!(deleted);
    let mems = db.list_memories("", false, "").unwrap();
    assert!(mems.is_empty());

    // delete non-existent
    let deleted = db.delete_memory(9999).unwrap();
    assert!(!deleted);
}

// ---------------------------------------------------------------------------
// 6. Memory staleness
// ---------------------------------------------------------------------------
#[test]
fn test_memory_staleness() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "", "", 1, 1, None)
        .unwrap();

    let mem_id = db
        .save_memory("important note", "note", &[sym_id])
        .unwrap();

    // Initially not stale
    let mems = db.get_memories_for_symbol(sym_id, false).unwrap();
    assert_eq!(mems.len(), 1);
    assert!(!mems[0].stale);

    // Mark stale
    let count = db.mark_memories_stale_for_file(file_id).unwrap();
    assert_eq!(count, 1);

    // Without include_stale: empty
    let mems = db.get_memories_for_symbol(sym_id, false).unwrap();
    assert!(mems.is_empty());

    // With include_stale: shows up, stale=true
    let mems = db.get_memories_for_symbol(sym_id, true).unwrap();
    assert_eq!(mems.len(), 1);
    assert!(mems[0].stale);

    // list_memories include_stale=false hides it, include_stale=true shows it
    let mems = db.list_memories("", false, "").unwrap();
    assert!(mems.is_empty());
    let mems = db.list_memories("", true, "").unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].id, mem_id);
}

// ---------------------------------------------------------------------------
// 7. FTS search
// ---------------------------------------------------------------------------
#[test]
fn test_fts_search() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();

    db.insert_symbol(
        file_id,
        "calculate_total",
        "function",
        "fn calculate_total(items: &[Item]) -> f64",
        "fn calculate_total(items: &[Item]) -> f64 { items.iter().map(|i| i.price).sum() }",
        1,
        3,
        None,
    )
    .unwrap();

    db.insert_symbol(
        file_id,
        "parse_config",
        "function",
        "fn parse_config(path: &str) -> Config",
        "fn parse_config(path: &str) -> Config { toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() }",
        5,
        8,
        None,
    )
    .unwrap();

    db.rebuild_fts().unwrap();

    // Search by name
    let results = db.search_code("calculate_total", "", None, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "calculate_total");

    // Search by body content
    let results = db.search_code("price", "", None, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "calculate_total");

    // Search by signature content
    let results = db.search_code("Config", "", None, 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "parse_config");

    // Search with kind filter
    let results = db
        .search_code("calculate_total", "function", None, 10)
        .unwrap();
    assert_eq!(results.len(), 1);

    // Search with repo_id filter
    let results = db
        .search_code("calculate_total", "", Some(repo_id), 10)
        .unwrap();
    assert_eq!(results.len(), 1);

    // No match
    let results = db.search_code("nonexistent_xyz", "", None, 10).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// 8. Auto-observation
// ---------------------------------------------------------------------------
#[test]
fn test_auto_observation() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "", "", 1, 1, None)
        .unwrap();

    let mem_id = db
        .save_auto_observation(
            "user refactored this function for clarity",
            "watcher",
            "session-abc",
            &[sym_id],
        )
        .unwrap();
    assert!(mem_id > 0);

    // Category is always "observation" for auto observations
    let mems = db.list_memories("observation", false, "").unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].content, "user refactored this function for clarity");
}

// ---------------------------------------------------------------------------
// 9. Transaction rollback on error
// ---------------------------------------------------------------------------
#[test]
fn test_transaction_rollback_on_error() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test", "/test").unwrap();

    let result = db.with_transaction(|| -> anyhow::Result<()> {
        db.upsert_file(repo_id, "a.go", "go", "hash_a")?;
        anyhow::bail!("intentional failure");
    });
    assert!(result.is_err());

    // File should NOT exist since transaction rolled back
    let files = db.get_files_for_repo(repo_id).unwrap();
    assert!(files.is_empty(), "rolled-back file should not persist");
}

// ---------------------------------------------------------------------------
// 10. Cleanup old auto-observations, keep manual
// ---------------------------------------------------------------------------
#[test]
fn test_cleanup_old_observations() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "", "", 1, 1, None)
        .unwrap();

    // Insert an old auto-observation by manually setting created_at in the past
    db.save_auto_observation("old auto note", "watcher", "s1", &[sym_id])
        .unwrap();

    // Backdate it to 60 days ago
    db.save_memory("manual note", "note", &[sym_id]).unwrap();

    // Backdate the auto-observation
    // The auto-observation is the first memory inserted, the manual is second.
    // We need to update created_at for the auto one.
    // Use a raw query via the internal conn — but Database doesn't expose conn.
    // Instead, we can use save_auto_observation and then cleanup with max_age_days=0
    // which deletes anything before "now - 0 days" = now (but created_at is also now).
    // That won't work. Let's use a different approach:
    // Insert auto-observation, then cleanup with max_age_days = -1 (future cutoff)
    // to delete everything.

    // Actually, let's think about this differently. We have 2 memories:
    // 1. auto (source='watcher') created at 'now'
    // 2. manual (source='manual') created at 'now'
    //
    // cleanup_old_auto_observations(0) deletes auto where created_at < datetime('now', '-0 days')
    // = datetime('now'). Since created_at is also datetime('now'), it's NOT less than,
    // so nothing gets deleted.
    //
    // To properly test, we need to backdate the auto memory.
    // Let's get the list and update directly. But we don't expose raw conn.
    //
    // Workaround: open a second connection to the same in-memory DB? No, in-memory DBs
    // aren't shared. Better: add the backdating to the test by using open() with a tmpfile.

    // Drop the in-memory approach — use a temp file and raw rusqlite to backdate.
    // Actually, the cleanest approach: test cleanup_old_auto_observations by ensuring
    // it targets the right source column and the manual memory survives.
    //
    // We'll create a fresh DB, insert both, backdate the auto one via a second
    // rusqlite connection, then call cleanup.

    // Start fresh with a temp file
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = Database::open(db_path.to_str().unwrap()).unwrap();

    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "", "", 1, 1, None)
        .unwrap();

    let auto_id = db
        .save_auto_observation("old auto", "watcher", "s1", &[sym_id])
        .unwrap();
    let _manual_id = db.save_memory("manual note", "note", &[sym_id]).unwrap();

    // Backdate the auto-observation using a separate connection
    {
        let conn = rusqlite::Connection::open(db_path.to_str().unwrap()).unwrap();
        conn.execute(
            "UPDATE memories SET created_at = datetime('now', '-90 days') WHERE id = ?1",
            rusqlite::params![auto_id],
        )
        .unwrap();
    }

    // Cleanup anything older than 30 days
    let cleaned = db.cleanup_old_auto_observations(30).unwrap();
    assert_eq!(cleaned, 1);

    // Manual memory survives
    let mems = db.list_memories("", false, "").unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].content, "manual note");
}

// ---------------------------------------------------------------------------
// 11. Incremental FTS — searchable on insert, vanishes on delete
// ---------------------------------------------------------------------------
#[test]
fn test_incremental_fts() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test", "/test").unwrap();
    let file_id = db.upsert_file(repo_id, "a.go", "go", "hash").unwrap();

    // Insert symbols — should be searchable immediately without rebuild_fts
    let _s1 = db
        .insert_symbol(
            file_id,
            "HandleRequest",
            "function",
            "func HandleRequest()",
            "func HandleRequest() {}",
            1,
            3,
            None,
        )
        .unwrap();

    let results = db.search_code("HandleRequest", "", None, 10).unwrap();
    assert_eq!(results.len(), 1, "symbol should be FTS-searchable after insert");

    // Delete symbols — should vanish from FTS
    db.delete_symbols_by_file(file_id).unwrap();
    let results = db.search_code("HandleRequest", "", None, 10).unwrap();
    assert!(results.is_empty(), "symbol should vanish from FTS after delete");
}

// ---------------------------------------------------------------------------
// 12. Duplicate edge prevention (UNIQUE constraint + INSERT OR IGNORE)
// ---------------------------------------------------------------------------
#[test]
fn test_duplicate_edge_ignored() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test", "/test").unwrap();
    let file_id = db.upsert_file(repo_id, "a.go", "go", "hash").unwrap();
    let s1 = db
        .insert_symbol(file_id, "A", "function", "fn A()", "", 1, 5, None)
        .unwrap();
    let s2 = db
        .insert_symbol(file_id, "B", "function", "fn B()", "", 6, 10, None)
        .unwrap();

    let e1 = db.insert_edge(s1, s2, "calls").unwrap();
    let e2 = db.insert_edge(s1, s2, "calls").unwrap(); // duplicate — should be ignored
    // First insert created a real row
    assert!(e1 > 0);
    // Second insert was ignored; last_insert_rowid is stale but no error
    let _ = e2;
    // Only one edge should exist
    let deps = db.get_dependencies(s1).unwrap();
    assert_eq!(deps.len(), 1);
}

// ---------------------------------------------------------------------------
// 13. Memory FTS search
// ---------------------------------------------------------------------------
#[test]
fn test_search_memories() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("test", "/test").unwrap();
    let file_id = db.upsert_file(repo_id, "a.go", "go", "hash").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "Foo", "function", "fn Foo()", "", 1, 5, None)
        .unwrap();

    db.save_memory(
        "architectural decision about caching layer",
        "architecture",
        &[sym_id],
    )
    .unwrap();
    db.save_memory("bug fix for race condition in handler", "bug", &[])
        .unwrap();

    let results = db.search_memories("caching", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("caching"));

    let results = db.search_memories("race condition", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("race condition"));

    // No results for non-matching query
    let results = db.search_memories("nonexistent_xyz", 10).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// 14. Session recovery — basic
// ---------------------------------------------------------------------------
#[test]
fn test_session_recovery_basic() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "src/main.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "handle_request", "function", "fn handle_request()", "", 1, 10, None)
        .unwrap();

    // Manual memory (cross-session, source='manual')
    db.save_memory("Use connection pooling for DB access", "decision", &[sym_id])
        .unwrap();

    // Auto-observation with symbol link
    db.save_auto_observation(
        "Explored 'handle_request' (1 results)",
        "auto:query_symbol",
        "session-100",
        &[sym_id],
    )
    .unwrap();

    // Auto-observation without symbol link
    db.save_auto_observation(
        "Context capsule for 'fix bug' (3 items, 5000 tokens)",
        "auto:get_context",
        "session-100",
        &[],
    )
    .unwrap();

    let data = db.get_session_recovery("session-100").unwrap();

    // Manual memories returned (cross-session)
    assert_eq!(data.manual_memories.len(), 1);
    assert_eq!(data.manual_memories[0].content, "Use connection pooling for DB access");

    // Auto-observations for session-100 only
    assert_eq!(data.auto_observations.len(), 2);

    // Recent files derived via memory_symbols → symbols → files
    assert_eq!(data.recent_files.len(), 1);
    assert_eq!(data.recent_files[0], "src/main.rs");

    // Symbol names from linked observations
    assert_eq!(data.symbol_names_accessed.len(), 1);
    assert_eq!(data.symbol_names_accessed[0], "handle_request");
}

// ---------------------------------------------------------------------------
// 15. Session recovery — empty session
// ---------------------------------------------------------------------------
#[test]
fn test_session_recovery_empty_session() {
    let db = Database::open_in_memory().unwrap();

    // Save a manual memory so we can verify it still shows up
    db.save_memory("Global architecture note", "architecture", &[])
        .unwrap();

    let data = db.get_session_recovery("session-nonexistent").unwrap();

    // Manual memories are cross-session
    assert_eq!(data.manual_memories.len(), 1);
    assert_eq!(data.manual_memories[0].content, "Global architecture note");

    // No session-specific data
    assert!(data.auto_observations.is_empty());
    assert!(data.recent_files.is_empty());
    assert!(data.symbol_names_accessed.is_empty());
}

// ---------------------------------------------------------------------------
// 16. Session recovery — session isolation
// ---------------------------------------------------------------------------
#[test]
fn test_session_recovery_session_isolation() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "a.rs", "rust", "h").unwrap();
    let sym_a = db
        .insert_symbol(file_id, "alpha", "function", "fn alpha()", "", 1, 5, None)
        .unwrap();
    let sym_b = db
        .insert_symbol(file_id, "beta", "function", "fn beta()", "", 6, 10, None)
        .unwrap();

    // Session 1 touches alpha
    db.save_auto_observation("Explored alpha", "auto:query_symbol", "session-1", &[sym_a])
        .unwrap();

    // Session 2 touches beta
    db.save_auto_observation("Explored beta", "auto:query_symbol", "session-2", &[sym_b])
        .unwrap();

    let data_1 = db.get_session_recovery("session-1").unwrap();
    assert_eq!(data_1.auto_observations.len(), 1);
    assert!(data_1.auto_observations[0].content.contains("alpha"));
    assert_eq!(data_1.symbol_names_accessed, vec!["alpha"]);

    let data_2 = db.get_session_recovery("session-2").unwrap();
    assert_eq!(data_2.auto_observations.len(), 1);
    assert!(data_2.auto_observations[0].content.contains("beta"));
    assert_eq!(data_2.symbol_names_accessed, vec!["beta"]);
}

// ---------------------------------------------------------------------------
// 17. Session recovery — stale observations excluded
// ---------------------------------------------------------------------------
#[test]
fn test_session_recovery_stale_excluded() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "stale_fn", "function", "fn stale_fn()", "", 1, 5, None)
        .unwrap();

    db.save_auto_observation("Explored stale_fn", "auto:query_symbol", "session-x", &[sym_id])
        .unwrap();

    // Mark memory stale (simulates file being re-indexed)
    db.mark_memories_stale_for_file(file_id).unwrap();

    let data = db.get_session_recovery("session-x").unwrap();
    assert!(data.auto_observations.is_empty(), "stale observations should be excluded");
    assert!(data.recent_files.is_empty(), "stale-linked files should not appear");
}

// ---------------------------------------------------------------------------
// 18. Session recovery — file dedup
// ---------------------------------------------------------------------------
#[test]
fn test_session_recovery_file_dedup() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "src/lib.rs", "rust", "h").unwrap();
    let sym_1 = db
        .insert_symbol(file_id, "foo", "function", "fn foo()", "", 1, 5, None)
        .unwrap();
    let sym_2 = db
        .insert_symbol(file_id, "bar", "function", "fn bar()", "", 6, 10, None)
        .unwrap();

    // Two different observations linking to different symbols in the SAME file
    db.save_auto_observation("Explored foo", "auto:query_symbol", "session-d", &[sym_1])
        .unwrap();
    db.save_auto_observation("Searched bar", "auto:search_code", "session-d", &[sym_2])
        .unwrap();

    let data = db.get_session_recovery("session-d").unwrap();

    // File appears only once despite two observations linking to symbols in it
    assert_eq!(data.recent_files.len(), 1);
    assert_eq!(data.recent_files[0], "src/lib.rs");

    // Both symbol names should appear
    assert_eq!(data.symbol_names_accessed.len(), 2);
    assert!(data.symbol_names_accessed.contains(&"bar".to_string()));
    assert!(data.symbol_names_accessed.contains(&"foo".to_string()));
}
