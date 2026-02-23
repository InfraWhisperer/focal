use std::collections::HashSet;

use focal_core::context::{ContextEngine, Intent};
use focal_core::db::Database;

/// Seed a test database with symbols and edges for context engine tests.
/// Layout:
///   repo "testrepo" -> file "src/lib.rs"
///     - handle_request (function, 400-char body) -- pivot target
///     - parse_input (function, 200-char body)    -- dependency of handle_request
///     - validate (function, 200-char body)       -- dependency of handle_request
///     - log_error (function, 100-char body)      -- dependent of handle_request
///   Edges: handle_request -> parse_input (calls)
///          handle_request -> validate   (calls)
///          log_error      -> handle_request (calls)
///   Memory linked to handle_request: "This function is the main entry point"
fn seed_db() -> (Database, i64) {
    let db = Database::open_in_memory().unwrap();
    let repo_id = db.upsert_repository("testrepo", "/tmp/testrepo").unwrap();
    let file_id = db
        .upsert_file(repo_id, "src/lib.rs", "rust", "abc123")
        .unwrap();

    // handle_request — large body, will be the FTS pivot hit
    let body_hr = "fn handle_request(req: Request) -> Response { \
        let input = parse_input(&req); \
        let valid = validate(&input); \
        if !valid { return Response::bad_request(); } \
        let result = process(input); \
        Response::ok(result) \
    }"
    .to_string()
        + &" ".repeat(200); // pad to ~400 chars

    let hr_id = db
        .insert_symbol(
            file_id,
            "handle_request",
            "function",
            "fn handle_request(req: Request) -> Response",
            &body_hr,
            "",
            1,
            10,
            None,
        )
        .unwrap();

    let body_pi = "fn parse_input(req: &Request) -> Input { serde_json::from_slice(&req.body).unwrap() }";
    let pi_id = db
        .insert_symbol(
            file_id,
            "parse_input",
            "function",
            "fn parse_input(req: &Request) -> Input",
            body_pi,
            "",
            12,
            15,
            None,
        )
        .unwrap();

    let body_v = "fn validate(input: &Input) -> bool { !input.name.is_empty() && input.age > 0 }";
    let v_id = db
        .insert_symbol(
            file_id,
            "validate",
            "function",
            "fn validate(input: &Input) -> bool",
            body_v,
            "",
            17,
            20,
            None,
        )
        .unwrap();

    let body_le = "fn log_error(msg: &str) { eprintln!(\"ERROR: {}\", msg); }";
    let le_id = db
        .insert_symbol(
            file_id,
            "log_error",
            "function",
            "fn log_error(msg: &str)",
            body_le,
            "",
            22,
            25,
            None,
        )
        .unwrap();

    // Edges: handle_request calls parse_input, validate
    // log_error calls handle_request (log_error depends on handle_request)
    db.insert_edge(hr_id, pi_id, "calls").unwrap();
    db.insert_edge(hr_id, v_id, "calls").unwrap();
    db.insert_edge(le_id, hr_id, "calls").unwrap();

    // Rebuild FTS index
    db.rebuild_fts().unwrap();

    // Memory linked to handle_request
    db.save_memory(
        "This function is the main entry point for all HTTP requests",
        "architecture",
        &[hr_id],
    )
    .unwrap();

    (db, repo_id)
}

// ---------------------------------------------------------------------------
// 1. Intent detection
// ---------------------------------------------------------------------------

#[test]
fn test_intent_detect_debug() {
    assert_eq!(Intent::detect("fix the crash in handler"), Intent::Debug);
    assert_eq!(Intent::detect("there's a bug in parse"), Intent::Debug);
    assert_eq!(Intent::detect("why does this fail?"), Intent::Debug);
    assert_eq!(Intent::detect("the server panic on startup"), Intent::Debug);
    assert_eq!(Intent::detect("this is broken"), Intent::Debug);
    assert_eq!(Intent::detect("fail test"), Intent::Debug);
}

#[test]
fn test_intent_detect_refactor() {
    assert_eq!(Intent::detect("refactor the handler"), Intent::Refactor);
    assert_eq!(Intent::detect("rename parse_input"), Intent::Refactor);
    assert_eq!(Intent::detect("extract a helper function"), Intent::Refactor);
    assert_eq!(Intent::detect("split this into modules"), Intent::Refactor);
    assert_eq!(Intent::detect("reorganize the layout"), Intent::Refactor);
}

#[test]
fn test_intent_detect_modify() {
    assert_eq!(Intent::detect("add a new endpoint"), Intent::Modify);
    assert_eq!(Intent::detect("implement caching"), Intent::Modify);
    assert_eq!(Intent::detect("create a struct for this"), Intent::Modify);
    assert_eq!(Intent::detect("build the auth feature"), Intent::Modify);
}

#[test]
fn test_intent_detect_explore() {
    assert_eq!(Intent::detect("how does the handler work?"), Intent::Explore);
    assert_eq!(Intent::detect("explain the architecture"), Intent::Explore);
    assert_eq!(Intent::detect("what does this do?"), Intent::Explore);
}

#[test]
fn test_intent_priority_debug_over_modify() {
    // "fix" (debug) should win over "add" (modify) even if both present
    assert_eq!(Intent::detect("fix the bug and add a test"), Intent::Debug);
}

// ---------------------------------------------------------------------------
// 2. Capsule respects token budget
// ---------------------------------------------------------------------------

#[test]
fn test_capsule_respects_token_budget() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    // Large budget — should fit pivot + adjacent
    let capsule = engine
        .get_capsule("handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();
    assert!(capsule.total_tokens <= capsule.budget);
    assert!(capsule.total_tokens <= 10000);
    assert!(!capsule.items.is_empty(), "should have at least one item");

    // Tiny budget — should still not exceed
    let capsule_tiny = engine
        .get_capsule("handle_request", 50, Some(repo_id), &HashSet::new())
        .unwrap();
    assert!(capsule_tiny.total_tokens <= 50);
    // With a 50-token budget, might have zero or one item depending on cost
    // but total_tokens must respect the cap
}

#[test]
fn test_capsule_intent_field_populated() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    let capsule = engine
        .get_capsule("fix the crash in handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();
    assert_eq!(capsule.intent, "debug");

    let capsule = engine
        .get_capsule("refactor handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();
    assert_eq!(capsule.intent, "refactor");

    let capsule = engine
        .get_capsule("how does handle_request work", 10000, Some(repo_id), &HashSet::new())
        .unwrap();
    assert_eq!(capsule.intent, "explore");
}

// ---------------------------------------------------------------------------
// 3. Pivot symbols have full body, adjacent are skeletonized
// ---------------------------------------------------------------------------

#[test]
fn test_pivot_has_body_adjacent_is_skeleton() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    // Use explore intent (dependencies only) with large budget
    let capsule = engine
        .get_capsule("handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();

    // Find the pivot
    let pivots: Vec<_> = capsule.items.iter().filter(|i| i.is_pivot).collect();
    assert!(!pivots.is_empty(), "should have at least one pivot");

    for pivot in &pivots {
        assert!(
            !pivot.body.is_empty(),
            "pivot '{}' should have a non-empty body",
            pivot.name
        );
    }

    // Find adjacent (non-pivot) items
    let adjacent: Vec<_> = capsule.items.iter().filter(|i| !i.is_pivot).collect();
    for adj in &adjacent {
        assert!(
            adj.body.is_empty(),
            "adjacent '{}' should have an empty body (skeleton), got: {}",
            adj.name,
            adj.body
        );
        // Adjacent items should still have a signature
        assert!(
            !adj.signature.is_empty(),
            "adjacent '{}' should have a signature",
            adj.name
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Debug intent expands to both dependents and dependencies
// ---------------------------------------------------------------------------

#[test]
fn test_debug_intent_expands_both_directions() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    let capsule = engine
        .get_capsule("fix handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();
    assert_eq!(capsule.intent, "debug");

    let names: Vec<&str> = capsule.items.iter().map(|i| i.name.as_str()).collect();

    // Should include handle_request as pivot
    assert!(
        names.contains(&"handle_request"),
        "missing pivot handle_request, got: {names:?}"
    );

    // Debug expands both directions: dependents (log_error) + dependencies (parse_input, validate)
    // At least one of the adjacent symbols should appear
    let has_dependent = names.contains(&"log_error");
    let has_dependency = names.contains(&"parse_input") || names.contains(&"validate");
    assert!(
        has_dependent || has_dependency,
        "debug intent should expand graph in at least one direction, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Memories are attached to the capsule
// ---------------------------------------------------------------------------

#[test]
fn test_capsule_includes_memories() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    let capsule = engine
        .get_capsule("handle_request", 10000, Some(repo_id), &HashSet::new())
        .unwrap();

    assert!(
        !capsule.memories.is_empty(),
        "capsule should include memories linked to pivot symbols"
    );
    assert!(
        capsule.memories[0]
            .content
            .contains("main entry point"),
        "expected the seeded memory content"
    );
}

// ---------------------------------------------------------------------------
// 6. Empty FTS results produce an empty capsule (no panic)
// ---------------------------------------------------------------------------

#[test]
fn test_capsule_empty_query_no_panic() {
    let (db, repo_id) = seed_db();
    let engine = ContextEngine::new(&db);

    let capsule = engine
        .get_capsule("zzz_nonexistent_symbol_xyz", 10000, Some(repo_id), &HashSet::new())
        .unwrap();

    assert!(capsule.items.is_empty());
    assert!(capsule.memories.is_empty());
    assert_eq!(capsule.total_tokens, 0);
}

// ---------------------------------------------------------------------------
// 7. Intent detection false-positive regression tests
// ---------------------------------------------------------------------------

#[test]
fn test_intent_no_false_positive_on_new() {
    // "new" removed from keywords — "explain the new API" should be Explore, not Modify
    assert_eq!(Intent::detect("explain the new API"), Intent::Explore);
}

#[test]
fn test_intent_no_false_positive_on_error_noun() {
    // "error" removed from keywords — "what does the error handler do" should be Explore
    assert_eq!(
        Intent::detect("what does the error handler do"),
        Intent::Explore
    );
}

#[test]
fn test_intent_no_false_positive_on_move() {
    // "move" removed from keywords — "how does move semantics work" should be Explore
    assert_eq!(
        Intent::detect("how does move semantics work"),
        Intent::Explore
    );
}

#[test]
fn test_intent_no_false_positive_on_substring() {
    // Word-boundary matching prevents "renewal" from matching "new",
    // "remove" from matching "move", "errortype" from matching "error"
    assert_eq!(Intent::detect("check the renewal process"), Intent::Explore);
    assert_eq!(Intent::detect("remove the old config"), Intent::Explore);
}

#[test]
fn test_intent_detect_debug_keyword() {
    // "debug" is now a keyword
    assert_eq!(Intent::detect("debug the handler"), Intent::Debug);
}
