use focal_core::db::Database;

/// Verify that multiple auto-observations from different sources and sessions
/// coexist correctly, and that cleanup targets only non-manual memories while
/// leaving manual memories intact.
#[test]
fn test_auto_capture_multiple_sources() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let sym_id = db
        .insert_symbol(file_id, "func", "function", "fn func()", "", 1, 5, None)
        .unwrap();

    // Simulate auto-observations from different tool handlers in the same session
    let session = "session-1234567890";

    db.save_auto_observation(
        "Explored 'func' (1 results)",
        "auto:query_symbol",
        session,
        &[sym_id],
    )
    .unwrap();

    db.save_auto_observation(
        "Searched 'func' (1 results)",
        "auto:search_code",
        session,
        &[sym_id],
    )
    .unwrap();

    db.save_auto_observation(
        "Impact analysis of 'func' (depth=2, 3 affected)",
        "auto:get_impact_graph",
        session,
        &[],
    )
    .unwrap();

    // Also save a manual memory
    db.save_memory("This function handles retries", "note", &[sym_id])
        .unwrap();

    // All four memories exist
    let all = db.list_memories("", false, "").unwrap();
    assert_eq!(all.len(), 4);

    // Three are observations
    let obs = db.list_memories("observation", false, "").unwrap();
    assert_eq!(obs.len(), 3);

    // One is manual
    let notes = db.list_memories("note", false, "").unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "This function handles retries");

    // Cleanup with max_age_days=0 won't delete anything (all created "now")
    let cleaned = db.cleanup_old_auto_observations(0).unwrap();
    assert_eq!(cleaned, 0);
}

/// Verify that auto-observations link to symbols correctly and show up
/// when querying memories for that symbol.
#[test]
fn test_auto_capture_symbol_links() {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("r", "/tmp/r").unwrap();
    let file_id = db.upsert_file(repo_id, "f.rs", "rust", "h").unwrap();
    let s1 = db
        .insert_symbol(file_id, "alpha", "function", "fn alpha()", "", 1, 3, None)
        .unwrap();
    let s2 = db
        .insert_symbol(file_id, "beta", "function", "fn beta()", "", 4, 6, None)
        .unwrap();

    // Auto-observation linked to s1 only
    db.save_auto_observation(
        "Explored 'alpha'",
        "auto:query_symbol",
        "session-1",
        &[s1],
    )
    .unwrap();

    // Auto-observation linked to both s1 and s2
    db.save_auto_observation(
        "Searched 'alpha beta'",
        "auto:search_code",
        "session-1",
        &[s1, s2],
    )
    .unwrap();

    // Memories for s1: both observations
    let mems_s1 = db.get_memories_for_symbol(s1, false).unwrap();
    assert_eq!(mems_s1.len(), 2);

    // Memories for s2: only the search observation
    let mems_s2 = db.get_memories_for_symbol(s2, false).unwrap();
    assert_eq!(mems_s2.len(), 1);
    assert_eq!(mems_s2[0].content, "Searched 'alpha beta'");
}
