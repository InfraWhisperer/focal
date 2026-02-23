use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::ContextEngine;
use crate::db::{Database, Symbol, SymbolResult};
use crate::graph::GraphEngine;

// ---------------------------------------------------------------------------
// Parameter structs — each tool gets its own params type with doc comments
// that surface as descriptions in the MCP JSON Schema.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct QuerySymbolParams {
    /// Symbol name to search for (substring match)
    pub name: String,
    /// Optional symbol kind filter (e.g. "function", "struct", "method")
    pub kind: Option<String>,
    /// Optional repository name filter
    pub repo: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetDependenciesParams {
    /// Name of the symbol whose dependencies to retrieve
    pub symbol_name: String,
    /// Max traversal depth (1-3, default 1)
    pub depth: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetDependentsParams {
    /// Name of the symbol whose dependents to retrieve
    pub symbol_name: String,
    /// Max traversal depth (1-3, default 1)
    pub depth: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetFileSymbolsParams {
    /// File path (relative within the repo or absolute)
    pub file_path: String,
    /// Optional repository name filter
    pub repo: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveMemoryParams {
    /// The content of the memory (decision, insight, note)
    pub content: String,
    /// Category tag (e.g. "decision", "pattern", "bug", "architecture")
    pub category: String,
    /// Optional symbol names to link this memory to
    pub symbol_names: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListMemoriesParams {
    /// Filter by category
    pub category: Option<String>,
    /// Include stale memories (default false)
    pub include_stale: Option<bool>,
    /// Filter by linked symbol name
    pub symbol_name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeleteMemoryParams {
    /// ID of the memory to delete
    pub memory_id: i64,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateMemoryParams {
    /// ID of the memory to update
    pub memory_id: i64,
    /// New content (if provided)
    pub content: Option<String>,
    /// New category (if provided)
    pub category: Option<String>,
    /// New symbol names to link (replaces existing links)
    pub symbol_names: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    /// FTS5 search query
    pub query: String,
    /// Optional symbol kind filter
    pub kind: Option<String>,
    /// Optional repository name filter
    pub repo: Option<String>,
    /// Max results to return (default 20)
    pub max_results: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetRepoOverviewParams {
    /// Optional repository name (omit for all repos)
    pub repo: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetContextParams {
    /// Natural-language or keyword query describing what context is needed
    pub query: String,
    /// Approximate token budget for the context capsule (default 12000)
    pub max_tokens: Option<usize>,
    /// Optional repository name filter
    pub repo: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetSkeletonParams {
    /// File path (relative to repo root)
    pub file_path: String,
    /// Optional repository name
    pub repo: Option<String>,
    /// Detail level: minimal, standard, verbose (default: standard)
    pub detail: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetImpactGraphParams {
    /// Name of the symbol to analyze blast radius for
    pub symbol_name: String,
    /// Max traversal depth (1-5, default 2). Higher values find more transitive dependents but take longer.
    pub depth: Option<usize>,
    /// Optional repository name filter
    pub repo: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchMemoryParams {
    /// Full-text search query across memory content
    pub query: String,
    /// Max results (default 10)
    pub max_results: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct BatchQueryParams {
    /// List of symbol names to look up
    pub symbol_names: Vec<String>,
    /// Approximate token budget for all results combined (default 8000)
    pub max_tokens: Option<usize>,
    /// Whether to include full bodies (default true)
    pub include_body: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetHealthParams {}

#[derive(Deserialize, JsonSchema)]
pub struct RecoverSessionParams {
    /// Session ID to recover (e.g. "session-1708617600000").
    /// If omitted, recovers the current session.
    pub session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetSymbolHistoryParams {
    /// Symbol name to look up git history for
    pub symbol_name: String,
    /// Max commit entries to return (default 5)
    pub max_entries: Option<usize>,
    /// Optional repository name filter
    pub repo: Option<String>,
}

#[derive(Serialize)]
struct CommitEntry {
    hash: String,
    author: String,
    date: String,
    message: String,
}

#[derive(Serialize)]
struct SessionRecovery {
    session_id: String,
    summary: String,
    decisions: Vec<String>,
    recent_files: Vec<String>,
    symbols_previously_viewed: Vec<String>,
    observation_count: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchLogicFlowParams {
    /// Source symbol name (start of the path)
    pub from_symbol: String,
    /// Target symbol name (end of the path)
    pub to_symbol: String,
    /// Optional repository name filter
    pub repo: Option<String>,
    /// Maximum number of paths to return (default 3)
    pub max_paths: Option<usize>,
}

// ---------------------------------------------------------------------------
// Dependency graph traversal result
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DepNode {
    name: String,
    kind: String,
    signature: String,
    file_path: String,
    edge_kind: String,
    depth: u32,
}

// ---------------------------------------------------------------------------
// Recovery summary builder
// ---------------------------------------------------------------------------

/// Build a human-readable recovery summary from session data.
/// Prioritizes manual memories (explicit decisions) over auto-observations
/// (tool usage logs). Groups observations by tool, caps file/symbol lists.
fn build_recovery_summary(data: &crate::db::SessionRecoveryData) -> String {
    use std::collections::BTreeMap;

    let mut parts: Vec<String> = Vec::new();

    // Section 1: Manual decisions (highest signal)
    if !data.manual_memories.is_empty() {
        parts.push(format!(
            "Stored decisions/notes ({}):",
            data.manual_memories.len()
        ));
        for m in &data.manual_memories {
            parts.push(format!("  - [{}] {}", m.category, m.content));
        }
    }

    // Section 2: Session activity (grouped by tool, last observation per tool)
    if !data.auto_observations.is_empty() {
        let mut by_tool: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for obs in &data.auto_observations {
            let tool = obs.source.strip_prefix("auto:").unwrap_or(&obs.source);
            by_tool
                .entry(tool.to_string())
                .or_default()
                .push(obs.content.clone());
        }

        parts.push(format!(
            "\nSession activity ({} tool calls):",
            data.auto_observations.len()
        ));
        for (tool, contents) in &by_tool {
            if let Some(last) = contents.last() {
                parts.push(format!("  - {}: {}", tool, last));
            }
        }
    }

    // Section 3: Files accessed (capped at 20)
    if !data.recent_files.is_empty() {
        parts.push(format!("\nFiles accessed ({}):", data.recent_files.len()));
        for f in data.recent_files.iter().take(20) {
            parts.push(format!("  - {}", f));
        }
        if data.recent_files.len() > 20 {
            parts.push(format!(
                "  ... and {} more",
                data.recent_files.len() - 20
            ));
        }
    }

    // Section 4: Symbols viewed (capped at 30)
    if !data.symbol_names_accessed.is_empty() {
        parts.push(format!(
            "\nSymbols previously viewed ({}) — bodies will be re-sent on next request:",
            data.symbol_names_accessed.len()
        ));
        for s in data.symbol_names_accessed.iter().take(30) {
            parts.push(format!("  - {}", s));
        }
        if data.symbol_names_accessed.len() > 30 {
            parts.push(format!(
                "  ... and {} more",
                data.symbol_names_accessed.len() - 30
            ));
        }
    }

    if parts.is_empty() {
        "No session data found. This may be a fresh session with no prior tool usage.".to_string()
    } else {
        parts.join("\n")
    }
}

// ---------------------------------------------------------------------------
// FocalServer
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FocalServer {
    db: Arc<Mutex<Database>>,
    #[allow(dead_code)]
    workspace_roots: Vec<PathBuf>,
    session_id: String,
    /// Symbol IDs whose full bodies have already been sent in this session.
    /// On subsequent requests, these symbols get skeleton + placeholder note
    /// instead of the full body, saving ~95% tokens on repeated lookups.
    sent_symbols: Arc<Mutex<HashSet<i64>>>,
    tool_router: ToolRouter<Self>,
}

impl FocalServer {
    pub fn new(db: Arc<Mutex<Database>>, workspace_roots: Vec<PathBuf>) -> Self {
        let session_id = format!(
            "session-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        Self {
            db,
            workspace_roots,
            session_id,
            sent_symbols: Arc::new(Mutex::new(HashSet::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// Resolve a list of symbol names to their IDs. Unknown names are silently skipped.
    fn resolve_symbol_ids(db: &Database, names: &[String]) -> Vec<i64> {
        let mut ids = Vec::new();
        for name in names {
            if let Ok(Some(sym)) = db.find_symbol_by_name_any(name) {
                ids.push(sym.id);
            }
        }
        ids
    }

    /// Walk the dependency graph breadth-first up to `max_depth` levels.
    /// `direction` selects outgoing (dependencies) or incoming (dependents).
    fn traverse_graph(
        db: &Database,
        start_name: &str,
        max_depth: u32,
        direction: GraphDirection,
    ) -> Result<Vec<DepNode>, String> {
        let sym = db
            .find_symbol_by_name_any(start_name)
            .map_err(|e| format!("db error: {e}"))?
            .ok_or_else(|| format!("symbol '{start_name}' not found"))?;

        let mut visited = HashSet::new();
        visited.insert(sym.id);
        let mut queue: VecDeque<(i64, u32)> = VecDeque::new();
        queue.push_back((sym.id, 0));
        let mut results = Vec::new();

        while let Some((current_id, current_depth)) = queue.pop_front() {
            if current_depth >= max_depth {
                continue;
            }

            let edges = match direction {
                GraphDirection::Dependencies => db.get_dependencies(current_id),
                GraphDirection::Dependents => db.get_dependents(current_id),
            }
            .map_err(|e| format!("db error: {e}"))?;

            for (edge, dep_sym) in edges {
                if visited.insert(dep_sym.id) {
                    let file_path = db
                        .get_file_path_for_symbol(dep_sym.id)
                        .unwrap_or_else(|_| "<unknown>".to_string());

                    results.push(DepNode {
                        name: dep_sym.name.clone(),
                        kind: dep_sym.kind.clone(),
                        signature: dep_sym.signature.clone(),
                        file_path,
                        edge_kind: edge.kind.clone(),
                        depth: current_depth + 1,
                    });

                    queue.push_back((dep_sym.id, current_depth + 1));
                }
            }
        }

        Ok(results)
    }

    /// Enrich raw `Symbol` records with file paths and linked memories.
    /// Uses a single batch query for memories instead of per-symbol lookups.
    fn enrich_symbols(db: &Database, symbols: &[Symbol]) -> Vec<SymbolResult> {
        let sym_ids: Vec<i64> = symbols.iter().map(|s| s.id).collect();
        let mem_map = db.get_memories_for_symbols_batch(&sym_ids, false).unwrap_or_default();

        symbols
            .iter()
            .map(|sym| {
                let file_path = db
                    .get_file_path_for_symbol(sym.id)
                    .unwrap_or_else(|_| "<unknown>".to_string());
                let memories = mem_map.get(&sym.id).cloned().unwrap_or_default();
                SymbolResult {
                    id: sym.id,
                    name: sym.name.clone(),
                    kind: sym.kind.clone(),
                    signature: sym.signature.clone(),
                    body: sym.body.clone(),
                    file_path,
                    repo_name: String::new(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                    memories,
                    dependency_hints: Vec::new(),
                }
            })
            .collect()
    }
}

enum GraphDirection {
    Dependencies,
    Dependents,
}

// ---------------------------------------------------------------------------
// Tool definitions — #[tool_router] generates Self::tool_router()
// ---------------------------------------------------------------------------

#[tool_router]
impl FocalServer {
    #[tool(description = "Look up symbols by name, optionally filtered by kind and repository. Returns full symbol details including signature, body, file path, and linked memories.")]
    fn query_symbol(
        &self,
        Parameters(params): Parameters<QuerySymbolParams>,
    ) -> Result<String, String> {
        let results = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let name = params.name.as_str();
            let kind = params.kind.as_deref().unwrap_or("");
            let repo = params.repo.as_deref().unwrap_or("");

            let results = db
                .query_symbols_full(name, kind, repo)
                .map_err(|e| format!("query error: {e}"))?;

            if !results.is_empty() {
                let sym_ids: Vec<i64> = results.iter().map(|r| r.id).collect();
                let _ = db.save_auto_observation(
                    &format!("Explored '{}' ({} results)", params.name, results.len()),
                    "auto:query_symbol",
                    &self.session_id,
                    &sym_ids,
                );
            }

            results
        };

        // Record symbol IDs as sent (full bodies were included)
        if let Ok(mut sent) = self.sent_symbols.lock() {
            for r in &results {
                sent.insert(r.id);
            }
        }

        serde_json::to_string_pretty(&results).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Get symbols that this symbol depends on (outgoing edges in the dependency graph). Traverses up to `depth` levels (max 3).")]
    fn get_dependencies(
        &self,
        Parameters(params): Parameters<GetDependenciesParams>,
    ) -> Result<String, String> {
        let nodes = {
            let max_depth = params.depth.unwrap_or(1).min(3);
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let nodes = Self::traverse_graph(
                &db,
                &params.symbol_name,
                max_depth,
                GraphDirection::Dependencies,
            )?;

            if !nodes.is_empty() {
                let _ = db.save_auto_observation(
                    &format!(
                        "Traversed dependencies of '{}' (depth={}, {} nodes)",
                        params.symbol_name, max_depth, nodes.len()
                    ),
                    "auto:get_dependencies",
                    &self.session_id,
                    &[],
                );
            }

            nodes
        };
        serde_json::to_string_pretty(&nodes).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Get symbols that depend on this symbol (incoming edges in the dependency graph). Traverses up to `depth` levels (max 3).")]
    fn get_dependents(
        &self,
        Parameters(params): Parameters<GetDependentsParams>,
    ) -> Result<String, String> {
        let nodes = {
            let max_depth = params.depth.unwrap_or(1).min(3);
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let nodes = Self::traverse_graph(
                &db,
                &params.symbol_name,
                max_depth,
                GraphDirection::Dependents,
            )?;

            if !nodes.is_empty() {
                let _ = db.save_auto_observation(
                    &format!(
                        "Traversed dependents of '{}' (depth={}, {} nodes)",
                        params.symbol_name, max_depth, nodes.len()
                    ),
                    "auto:get_dependents",
                    &self.session_id,
                    &[],
                );
            }

            nodes
        };
        serde_json::to_string_pretty(&nodes).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "List all symbols in a file (signatures only, no bodies). Useful for understanding file structure without consuming token budget on full source.")]
    fn get_file_symbols(
        &self,
        Parameters(params): Parameters<GetFileSymbolsParams>,
    ) -> Result<String, String> {
        let summaries = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            db.get_file_symbols_summary(&params.file_path, params.repo.as_deref())
                .map_err(|e| format!("query error: {e}"))?
        };
        serde_json::to_string_pretty(&summaries).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Store a decision, insight, or architectural note as a persistent memory. Optionally link it to specific symbols so it surfaces in future context lookups.")]
    fn save_memory(
        &self,
        Parameters(params): Parameters<SaveMemoryParams>,
    ) -> Result<String, String> {
        let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
        let symbol_ids = params
            .symbol_names
            .as_ref()
            .map(|names| Self::resolve_symbol_ids(&db, names))
            .unwrap_or_default();

        let id = db
            .save_memory(&params.content, &params.category, &symbol_ids)
            .map_err(|e| format!("save error: {e}"))?;

        Ok(format!("{{\"memory_id\": {id}}}"))
    }

    #[tool(description = "List stored memories, optionally filtered by category, staleness, or linked symbol name.")]
    fn list_memories(
        &self,
        Parameters(params): Parameters<ListMemoriesParams>,
    ) -> Result<String, String> {
        let memories = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let category = params.category.as_deref().unwrap_or("");
            let include_stale = params.include_stale.unwrap_or(false);
            let symbol_name = params.symbol_name.as_deref().unwrap_or("");

            db.list_memories(category, include_stale, symbol_name)
                .map_err(|e| format!("query error: {e}"))?
        };
        serde_json::to_string_pretty(&memories).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Delete a memory by its ID.")]
    fn delete_memory(
        &self,
        Parameters(params): Parameters<DeleteMemoryParams>,
    ) -> Result<String, String> {
        let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
        let deleted = db
            .delete_memory(params.memory_id)
            .map_err(|e| format!("delete error: {e}"))?;

        if deleted {
            Ok(format!("{{\"deleted\": true, \"memory_id\": {}}}", params.memory_id))
        } else {
            Err(format!("memory {} not found", params.memory_id))
        }
    }

    #[tool(description = "Update an existing memory's content, category, or symbol links. Only provided fields are changed; omitted fields keep their current values.")]
    fn update_memory(
        &self,
        Parameters(params): Parameters<UpdateMemoryParams>,
    ) -> Result<String, String> {
        let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;

        let current = db
            .get_memory_by_id(params.memory_id)
            .map_err(|e| format!("query error: {e}"))?
            .ok_or_else(|| format!("memory {} not found", params.memory_id))?;

        let content = params.content.as_deref().unwrap_or(&current.content);
        let category = params.category.as_deref().unwrap_or(&current.category);
        let symbol_ids = match &params.symbol_names {
            Some(names) => Self::resolve_symbol_ids(&db, names),
            None => db
                .get_symbol_ids_for_memory(params.memory_id)
                .unwrap_or_default(),
        };

        db.update_memory(params.memory_id, content, category, &symbol_ids)
            .map_err(|e| format!("update error: {e}"))?;

        Ok(format!("{{\"updated\": true, \"memory_id\": {}}}", params.memory_id))
    }

    #[tool(description = "Full-text search across all indexed symbol names, signatures, and bodies using SQLite FTS5. Returns matching symbols ranked by relevance.")]
    fn search_code(
        &self,
        Parameters(params): Parameters<SearchCodeParams>,
    ) -> Result<String, String> {
        let results = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let kind = params.kind.as_deref().unwrap_or("");
            let max_results = params.max_results.unwrap_or(20);

            // Resolve repo name to ID if provided
            let repo_id = if let Some(ref repo_name) = params.repo {
                db.get_repo_id_by_name(repo_name)
                    .map_err(|e| format!("repo lookup error: {e}"))?
            } else {
                None
            };

            let symbols = db
                .search_code(&params.query, kind, repo_id, max_results)
                .map_err(|e| format!("search error: {e}"))?;

            let results = Self::enrich_symbols(&db, &symbols);

            if !results.is_empty() {
                let sym_ids: Vec<i64> = results.iter().map(|r| r.id).collect();
                let _ = db.save_auto_observation(
                    &format!("Searched '{}' ({} results)", params.query, results.len()),
                    "auto:search_code",
                    &self.session_id,
                    &sym_ids,
                );
            }

            results
        };

        // Record symbol IDs as sent (full bodies were included)
        if let Ok(mut sent) = self.sent_symbols.lock() {
            for r in &results {
                sent.insert(r.id);
            }
        }

        serde_json::to_string_pretty(&results).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Full-text search across stored memories and observations. Finds memories by content, useful for recalling architectural decisions, patterns, and prior insights.")]
    fn search_memory(
        &self,
        Parameters(params): Parameters<SearchMemoryParams>,
    ) -> Result<String, String> {
        let results = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let max = params.max_results.unwrap_or(10);
            db.search_memories(&params.query, max)
                .map_err(|e| format!("search error: {e}"))?
        };
        serde_json::to_string_pretty(&results).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Get an overview of indexed repositories including file counts, symbol counts, memory counts, and language breakdown.")]
    fn get_repo_overview(
        &self,
        Parameters(params): Parameters<GetRepoOverviewParams>,
    ) -> Result<String, String> {
        let overview = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let repo_name = params.repo.as_deref().unwrap_or("");
            db.get_repo_overview(repo_name)
                .map_err(|e| format!("overview error: {e}"))?
        };
        serde_json::to_string_pretty(&overview).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Retrieve focused, token-budgeted context for a query. Detects intent (debug/refactor/modify/explore), finds pivot symbols via FTS5, expands to adjacent symbols via the dependency graph, and attaches relevant memories. Pivots include full bodies on first request; subsequent requests for the same symbols within this session return skeleton + note (progressive disclosure). Respects the token budget throughout.")]
    fn get_context(
        &self,
        Parameters(params): Parameters<GetContextParams>,
    ) -> Result<String, String> {
        let capsule = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let sent = self.sent_symbols.lock().map_err(|e| format!("lock error: {e}"))?;
            let max_tokens = params.max_tokens.unwrap_or(12_000);

            let repo_id = if let Some(ref repo_name) = params.repo {
                db.get_repo_id_by_name(repo_name)
                    .map_err(|e| format!("repo lookup error: {e}"))?
            } else {
                None
            };

            let engine = ContextEngine::new(&db);
            let capsule = engine
                .get_capsule(&params.query, max_tokens, repo_id, &sent)
                .map_err(|e| format!("context error: {e}"))?;

            if !capsule.items.is_empty() {
                let _ = db.save_auto_observation(
                    &format!(
                        "Context capsule for '{}' ({} items, {} tokens)",
                        params.query,
                        capsule.items.len(),
                        capsule.total_tokens
                    ),
                    "auto:get_context",
                    &self.session_id,
                    &[],
                );
            }

            capsule
        };

        // Record newly-sent symbol IDs (those with full bodies, not placeholders)
        {
            if let Ok(mut sent) = self.sent_symbols.lock() {
                for item in &capsule.items {
                    if item.is_pivot && !item.body.starts_with("(full body") {
                        sent.insert(item.symbol_id);
                    }
                }
            }
        }

        serde_json::to_string_pretty(&capsule).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Token-efficient file view: returns signatures and types without implementation bodies. 70-90% fewer tokens than full source.")]
    fn get_skeleton(
        &self,
        Parameters(params): Parameters<GetSkeletonParams>,
    ) -> Result<String, String> {
        let results = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let detail = params.detail.as_deref().unwrap_or("standard");
            db.get_skeleton_by_path(&params.file_path, params.repo.as_deref(), detail)
                .map_err(|e| format!("query error: {e}"))?
        };
        serde_json::to_string_pretty(&results).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Compute the blast radius of changing a symbol. Traverses reverse dependency edges (who depends on this?) via BFS, returning all transitively affected symbols up to `depth` hops away.")]
    fn get_impact_graph(
        &self,
        Parameters(params): Parameters<GetImpactGraphParams>,
    ) -> Result<String, String> {
        let nodes = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let max_depth = params.depth.unwrap_or(2).min(5);

            let repo_id = if let Some(ref repo_name) = params.repo {
                db.get_repo_id_by_name(repo_name)
                    .map_err(|e| format!("repo lookup error: {e}"))?
            } else {
                None
            };

            let engine = GraphEngine::new(&db);
            let nodes = engine
                .impact_graph(&params.symbol_name, max_depth, repo_id)
                .map_err(|e| format!("graph error: {e}"))?;

            if !nodes.is_empty() {
                let _ = db.save_auto_observation(
                    &format!(
                        "Impact analysis of '{}' (depth={}, {} affected)",
                        params.symbol_name, max_depth, nodes.len()
                    ),
                    "auto:get_impact_graph",
                    &self.session_id,
                    &[],
                );
            }

            nodes
        };
        serde_json::to_string_pretty(&nodes).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Find call/dependency paths between two symbols. Traverses forward dependency edges via BFS to discover how `from_symbol` reaches `to_symbol`. Returns up to `max_paths` distinct paths, each as an ordered list of symbol names.")]
    fn search_logic_flow(
        &self,
        Parameters(params): Parameters<SearchLogicFlowParams>,
    ) -> Result<String, String> {
        let result: Vec<Vec<String>> = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let max_paths = params.max_paths.unwrap_or(3);

            let repo_id = if let Some(ref repo_name) = params.repo {
                db.get_repo_id_by_name(repo_name)
                    .map_err(|e| format!("repo lookup error: {e}"))?
            } else {
                None
            };

            let engine = GraphEngine::new(&db);
            let paths = engine
                .find_paths(&params.from_symbol, &params.to_symbol, max_paths, repo_id)
                .map_err(|e| format!("graph error: {e}"))?;

            // Transform into owned strings; lock drops at end of block
            paths
                .into_iter()
                .map(|path| path.into_iter().map(|s| s.name).collect())
                .collect()
        };
        serde_json::to_string_pretty(&result).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Fetch multiple symbols in a single call within a token budget. More efficient than multiple query_symbol calls when you need several specific symbols. Includes dependency hints when a symbol implements a trait/interface or imports types not in the result set.")]
    fn batch_query(
        &self,
        Parameters(params): Parameters<BatchQueryParams>,
    ) -> Result<String, String> {
        let results = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let include_body = params.include_body.unwrap_or(true);
            let budget = params.max_tokens.unwrap_or(8000);
            let mut used = 0usize;
            let mut out: Vec<(crate::db::Symbol, String)> = Vec::new();

            // Phase 1: collect symbols within budget
            for name in &params.symbol_names {
                if let Ok(Some(sym)) = db.find_symbol_by_name_any(name) {
                    let file_path = db
                        .get_file_path_for_symbol(sym.id)
                        .unwrap_or_else(|_| "<unknown>".to_string());
                    let body_len = if include_body { sym.body.len() } else { 0 };
                    let cost =
                        (sym.name.len() + sym.signature.len() + file_path.len() + body_len + 20)
                            .div_ceil(4);
                    if used + cost > budget {
                        break;
                    }
                    used += cost;
                    out.push((sym, file_path));
                }
            }

            // Phase 2: batch-fetch memories for all collected symbols
            let sym_ids: Vec<i64> = out.iter().map(|(s, _)| s.id).collect();
            let sym_id_set: HashSet<i64> = sym_ids.iter().copied().collect();
            let mut mem_map = db
                .get_memories_for_symbols_batch(&sym_ids, false)
                .unwrap_or_default();

            // Phase 3: compute dependency hints — surface unseen interfaces/traits
            let mut hint_map: std::collections::HashMap<i64, Vec<String>> =
                std::collections::HashMap::new();
            for (sym, _) in &out {
                if let Ok(deps) = db.get_dependency_hint_names(sym.id, &sym_id_set) {
                    let mut hints = Vec::new();
                    for (dep_name, dep_kind, edge_kind) in deps {
                        // Only hint about symbols not already in the batch result
                        if out.iter().any(|(s, _)| s.name == dep_name) {
                            continue;
                        }
                        let relation = match edge_kind.as_str() {
                            "type_ref" => format!("References {dep_kind} `{dep_name}` (not in context)"),
                            "imports" => format!("Imports `{dep_name}` (not in context)"),
                            "calls" => format!("Calls `{dep_name}` (not in context)"),
                            _ => format!("Depends on `{dep_name}` (not in context)"),
                        };
                        hints.push(relation);
                    }
                    if !hints.is_empty() {
                        hint_map.insert(sym.id, hints);
                    }
                }
            }

            out.into_iter()
                .map(|(sym, file_path)| {
                    let memories = mem_map.remove(&sym.id).unwrap_or_default();
                    let dependency_hints = hint_map.remove(&sym.id).unwrap_or_default();
                    SymbolResult {
                        id: sym.id,
                        name: sym.name.clone(),
                        kind: sym.kind.clone(),
                        signature: sym.signature.clone(),
                        body: if include_body {
                            sym.body.clone()
                        } else {
                            String::new()
                        },
                        file_path,
                        repo_name: String::new(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        memories,
                        dependency_hints,
                    }
                })
                .collect::<Vec<_>>()
        };
        // Track sent symbols for progressive disclosure
        if let Ok(mut sent) = self.sent_symbols.lock() {
            for r in &results {
                sent.insert(r.id);
            }
        }
        serde_json::to_string_pretty(&results).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Report database health: size, row counts, FTS integrity. Useful for diagnosing index issues.")]
    fn get_health(
        &self,
        Parameters(_): Parameters<GetHealthParams>,
    ) -> Result<String, String> {
        let report = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            db.get_health()
                .map_err(|e| format!("health check error: {e}"))?
        };
        serde_json::to_string_pretty(&report).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Get git commit history for a specific symbol's file. Shows who last changed it and why. Requires git to be available in PATH.")]
    fn get_symbol_history(
        &self,
        Parameters(params): Parameters<GetSymbolHistoryParams>,
    ) -> Result<String, String> {
        let (file_path, repo_root) = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            let repo_id = if let Some(ref name) = params.repo {
                db.get_repo_id_by_name(name)
                    .map_err(|e| format!("repo error: {e}"))?
            } else {
                None
            };
            let sym = match repo_id {
                Some(rid) => db.find_symbol_by_name(rid, &params.symbol_name),
                None => db.find_symbol_by_name_any(&params.symbol_name),
            }
            .map_err(|e| format!("db error: {e}"))?
            .ok_or_else(|| format!("symbol '{}' not found", params.symbol_name))?;

            let fp = db
                .get_file_path_for_symbol(sym.id)
                .map_err(|e| format!("file path error: {e}"))?;

            let root = self
                .workspace_roots
                .first()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| "no workspace root configured".to_string())?;

            (fp, root)
        };

        let max = params.max_entries.unwrap_or(5);

        let output = std::process::Command::new("git")
            .args([
                "log",
                "--format=%H%n%an%n%aI%n%s%n---",
                &format!("-{max}"),
                "--",
                &file_path,
            ])
            .current_dir(&repo_root)
            .output()
            .map_err(|e| format!("git error: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git log failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let commits: Vec<CommitEntry> = stdout
            .split("---\n")
            .filter(|s| !s.trim().is_empty())
            .filter_map(|block| {
                let lines: Vec<&str> = block.trim().lines().collect();
                if lines.len() >= 4 {
                    Some(CommitEntry {
                        hash: lines[0].to_string(),
                        author: lines[1].to_string(),
                        date: lines[2].to_string(),
                        message: lines[3..].join(" "),
                    })
                } else {
                    None
                }
            })
            .collect();

        serde_json::to_string_pretty(&commits).map_err(|e| format!("json error: {e}"))
    }

    #[tool(description = "Recover session state after context compaction. Returns architectural decisions, recently accessed files, and symbols previously viewed. Call this after a context window reset to restore working memory. Resets progressive disclosure so previously-sent symbol bodies will be re-sent fresh on next request.")]
    fn recover_session(
        &self,
        Parameters(params): Parameters<RecoverSessionParams>,
    ) -> Result<String, String> {
        let target_session = params
            .session_id
            .as_deref()
            .unwrap_or(&self.session_id);

        let data = {
            let db = self.db.lock().map_err(|e| format!("lock error: {e}"))?;
            db.get_session_recovery(target_session)
                .map_err(|e| format!("recovery error: {e}"))?
        };

        // Reset sent_symbols — after compaction Claude doesn't have those
        // bodies in context anymore. Progressive disclosure would return
        // "(full body sent earlier in session)" for symbols Claude no longer
        // remembers, effectively hiding their content.
        {
            let mut sent = self
                .sent_symbols
                .lock()
                .map_err(|e| format!("lock error: {e}"))?;
            sent.clear();
        }

        let decisions: Vec<String> = data
            .manual_memories
            .iter()
            .map(|m| format!("[{}] {}", m.category, m.content))
            .collect();

        let summary = build_recovery_summary(&data);

        let recovery = SessionRecovery {
            session_id: data.session_id,
            summary,
            decisions,
            recent_files: data.recent_files,
            symbols_previously_viewed: data.symbol_names_accessed,
            observation_count: data.auto_observations.len(),
        };

        serde_json::to_string_pretty(&recovery).map_err(|e| format!("json error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler — #[tool_handler] wires call_tool + list_tools to the router
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for FocalServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "focal: structural code index for Claude Code. \
                 Query symbols, navigate dependency graphs, search code via FTS5, \
                 and store persistent memories about architectural decisions."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: rmcp::model::Implementation {
                name: "focal".to_string(),
                title: Some("Focal MCP Server".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
