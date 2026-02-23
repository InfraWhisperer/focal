use std::fs;

use tempfile::TempDir;
use focal_core::db::Database;
use focal_core::grammar::GrammarRegistry;
use focal_core::indexer::Indexer;

/// Helper: write a file into the temp directory at a relative path.
fn write_file(dir: &TempDir, rel_path: &str, content: &str) {
    let full = dir.path().join(rel_path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
}

/// End-to-end integration test exercising the full pipeline:
/// index → FTS → symbol query → memory save → staleness propagation.
#[test]
fn test_full_pipeline() {
    // ---------------------------------------------------------------
    // 1. Set up temp directory with Go source files
    // ---------------------------------------------------------------
    let dir = TempDir::new().unwrap();

    let server_go = r#"package main

import "fmt"

// Server handles incoming requests.
type Server struct {
    Port int
    Name string
}

// NewServer constructs a Server with sensible defaults.
func NewServer(port int) *Server {
    return &Server{Port: port, Name: "default"}
}

// Start begins listening on the configured port.
func Start(s *Server) {
    fmt.Println("starting server on", s.Port)
    HandleRequest(s)
}

// HandleRequest processes a single request.
func HandleRequest(s *Server) {
    fmt.Println("handling request for", s.Name)
}
"#;

    let utils_go = r#"package main

import "strings"

// Sanitize strips whitespace and lowercases input.
func Sanitize(input string) string {
    return strings.TrimSpace(strings.ToLower(input))
}

// FormatPort converts a port number to a display string.
func FormatPort(port int) string {
    return fmt.Sprintf(":%d", port)
}
"#;

    write_file(&dir, "server.go", server_go);
    write_file(&dir, "utils.go", utils_go);

    // ---------------------------------------------------------------
    // 2. Index using Database + GrammarRegistry + Indexer
    // ---------------------------------------------------------------
    let db = Database::open_in_memory().unwrap();
    let registry = GrammarRegistry::new();
    let indexer = Indexer::new(&db, &registry);

    let stats = indexer.index_directory(dir.path()).unwrap();

    assert_eq!(stats.files_indexed, 2, "expected 2 files indexed, got {}", stats.files_indexed);
    // Server: Server (struct), NewServer, Start, HandleRequest = 4
    // Utils: Sanitize, FormatPort = 2
    // Total >= 6 (parser may extract more depending on grammar impl)
    assert!(
        stats.symbols_extracted >= 5,
        "expected >= 5 symbols extracted, got {}",
        stats.symbols_extracted
    );
    assert!(stats.errors.is_empty(), "index errors: {:?}", stats.errors);

    // Resolve repo_id for subsequent queries
    let root = dir.path().canonicalize().unwrap();
    let root_str = root.to_string_lossy().to_string();
    let repo = db.get_repository_by_path(&root_str).unwrap()
        .expect("repository should exist after indexing");
    let repo_id = repo.id;

    // ---------------------------------------------------------------
    // 3. Rebuild FTS index
    // ---------------------------------------------------------------
    db.rebuild_fts().unwrap();

    // ---------------------------------------------------------------
    // 4. Query symbols by name
    // ---------------------------------------------------------------
    let sym = db.find_symbol_by_name(repo_id, "NewServer").unwrap()
        .expect("NewServer symbol should exist");
    assert_eq!(sym.name, "NewServer");
    assert_eq!(sym.kind, "function");

    let sym_start = db.find_symbol_by_name(repo_id, "Start").unwrap()
        .expect("Start symbol should exist");
    assert_eq!(sym_start.name, "Start");

    // Rich query — search by partial name, no kind/repo filter
    let results = db.query_symbols_full("Server", "", "").unwrap();
    assert!(
        !results.is_empty(),
        "query_symbols_full('Server') should return results"
    );
    let matched_names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
    assert!(
        matched_names.iter().any(|n| n.contains("Server")),
        "expected at least one symbol containing 'Server', got: {matched_names:?}"
    );

    // ---------------------------------------------------------------
    // 5. Full-text search via FTS
    // ---------------------------------------------------------------
    // Search by function name
    let fts_results = db.search_code("NewServer", "", None, 10).unwrap();
    assert!(
        !fts_results.is_empty(),
        "FTS search for 'NewServer' should return results"
    );
    assert_eq!(fts_results[0].name, "NewServer");

    // Search by body content — "starting server" appears in Start's body
    let fts_body = db.search_code("starting server", "", None, 10).unwrap();
    assert!(
        !fts_body.is_empty(),
        "FTS search for 'starting server' should match Start's body"
    );
    assert_eq!(fts_body[0].name, "Start");

    // Search with kind filter
    let fts_func = db.search_code("Sanitize", "function", None, 10).unwrap();
    assert_eq!(fts_func.len(), 1);
    assert_eq!(fts_func[0].name, "Sanitize");

    // Search scoped to repo
    let fts_repo = db.search_code("HandleRequest", "", Some(repo_id), 10).unwrap();
    assert!(
        !fts_repo.is_empty(),
        "FTS search scoped to repo should find HandleRequest"
    );

    // ---------------------------------------------------------------
    // 6. Save a memory linked to a symbol
    // ---------------------------------------------------------------
    let handle_sym = db.find_symbol_by_name(repo_id, "HandleRequest").unwrap()
        .expect("HandleRequest should exist");

    let memory_id = db.save_memory(
        "HandleRequest is the hot path — profile before optimizing",
        "note",
        &[handle_sym.id],
    ).unwrap();
    assert!(memory_id > 0);

    // ---------------------------------------------------------------
    // 7. Retrieve memories for that symbol
    // ---------------------------------------------------------------
    // Via get_memories_for_symbol
    let mems = db.get_memories_for_symbol(handle_sym.id, false).unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].content, "HandleRequest is the hot path — profile before optimizing");
    assert!(!mems[0].stale);

    // Via list_memories filtered by symbol name
    let mems_by_name = db.list_memories("", false, "HandleRequest").unwrap();
    assert_eq!(mems_by_name.len(), 1);
    assert_eq!(mems_by_name[0].id, memory_id);

    // Via list_memories filtered by category
    let mems_by_cat = db.list_memories("note", false, "").unwrap();
    assert_eq!(mems_by_cat.len(), 1);

    // Via query_symbols_full — memories should be attached to the symbol result
    let rich = db.query_symbols_full("HandleRequest", "", "").unwrap();
    assert!(!rich.is_empty());
    assert!(
        !rich[0].memories.is_empty(),
        "query_symbols_full should attach memories to HandleRequest"
    );

    // ---------------------------------------------------------------
    // 8. Simulate file change → staleness propagation
    //
    // Modify server.go content so the hash changes. The indexer calls
    // mark_memories_stale_for_file internally when it detects a hash
    // change and re-indexes the file.
    // ---------------------------------------------------------------
    let server_go_v2 = r#"package main

import "fmt"

// Server handles incoming requests — v2 with graceful shutdown.
type Server struct {
    Port     int
    Name     string
    Shutdown bool
}

// NewServer constructs a Server with sensible defaults.
func NewServer(port int) *Server {
    return &Server{Port: port, Name: "default", Shutdown: false}
}

// Start begins listening on the configured port.
func Start(s *Server) {
    fmt.Println("starting server on", s.Port)
    HandleRequest(s)
}

// HandleRequest processes a single request — updated signature.
func HandleRequest(s *Server) {
    if s.Shutdown {
        fmt.Println("server shutting down, rejecting request")
        return
    }
    fmt.Println("handling request for", s.Name)
}
"#;

    write_file(&dir, "server.go", server_go_v2);

    let stats2 = indexer.index_directory(dir.path()).unwrap();
    // server.go changed, utils.go unchanged
    assert_eq!(stats2.files_indexed, 1, "only server.go should be re-indexed");
    assert_eq!(stats2.files_skipped, 1, "utils.go should be skipped (unchanged)");

    // ---------------------------------------------------------------
    // 9. Verify memory re-linking after re-index
    // ---------------------------------------------------------------
    // HandleRequest still exists in v2, so the indexer re-links the memory
    // to the new symbol ID and clears the stale flag.
    let mems_fresh = db.list_memories("", false, "").unwrap();
    assert_eq!(
        mems_fresh.len(), 1,
        "memory should be re-linked and un-staled since HandleRequest persists, got {}",
        mems_fresh.len()
    );
    assert!(!mems_fresh[0].stale, "re-linked memory should not be stale");
    assert_eq!(mems_fresh[0].id, memory_id);

    // The new HandleRequest symbol should have the memory attached via re-linking.
    let new_handle = db.find_symbol_by_name(repo_id, "HandleRequest").unwrap()
        .expect("HandleRequest should exist after re-index");
    let mems_new_sym = db.get_memories_for_symbol(new_handle.id, false).unwrap();
    assert_eq!(
        mems_new_sym.len(), 1,
        "re-linked memory should attach to new HandleRequest symbol"
    );
    assert_eq!(mems_new_sym[0].id, memory_id);
}
