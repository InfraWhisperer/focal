use std::collections::HashSet;

use serde::Serialize;

use crate::db::{Database, Memory, Symbol};

// ---------------------------------------------------------------------------
// Intent detection
// ---------------------------------------------------------------------------

/// Detected intent from a natural-language query. Drives which graph edges
/// the context engine follows when expanding beyond pivot symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    /// "fix", "bug", "crash", "fail", "panic", "broken", "debug"
    Debug,
    /// "refactor", "rename", "extract", "split", "reorganize"
    Refactor,
    /// "add", "implement", "create", "build", "feature"
    Modify,
    /// Default — no strong signal from the query text
    Explore,
}

impl Intent {
    /// Detect intent from query text using word-boundary matching.
    /// Each category's keywords are counted; highest count wins.
    /// Ties go to priority order (Debug > Refactor > Modify).
    /// Returns Explore when no keywords match.
    pub fn detect(query: &str) -> Self {
        let lower = query.to_lowercase();
        // Strip trailing punctuation from each word so "fail?" matches "fail"
        let words: Vec<String> = lower
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .collect();

        const DEBUG_KEYWORDS: &[&str] = &["fix", "bug", "crash", "fail", "panic", "broken", "debug"];
        const REFACTOR_KEYWORDS: &[&str] = &["refactor", "rename", "extract", "split", "reorganize"];
        const MODIFY_KEYWORDS: &[&str] = &["add", "implement", "create", "build", "feature"];

        // Count keyword hits per category
        let debug_hits = words.iter().filter(|w| DEBUG_KEYWORDS.contains(&w.as_str())).count();
        let refactor_hits = words.iter().filter(|w| REFACTOR_KEYWORDS.contains(&w.as_str())).count();
        let modify_hits = words.iter().filter(|w| MODIFY_KEYWORDS.contains(&w.as_str())).count();

        // Highest count wins; ties go to priority order (Debug > Refactor > Modify)
        let max = debug_hits.max(refactor_hits).max(modify_hits);
        if max == 0 {
            return Self::Explore;
        }
        if debug_hits == max {
            return Self::Debug;
        }
        if refactor_hits == max {
            return Self::Refactor;
        }
        Self::Modify
    }
}

// ---------------------------------------------------------------------------
// Capsule data types
// ---------------------------------------------------------------------------

/// A single symbol packaged for the context capsule. Pivot symbols carry their
/// full body; adjacent (graph-expanded) symbols carry only the signature.
#[derive(Debug, Clone, Serialize)]
pub struct CapsuleItem {
    /// Database ID of the symbol. Used for session-aware progressive disclosure:
    /// once a symbol's body has been sent, subsequent requests can skip it.
    pub symbol_id: i64,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub signature: String,
    /// Full body for pivots, empty for adjacent symbols (skeleton mode).
    /// For symbols already sent in this session, contains a placeholder note.
    pub body: String,
    pub is_pivot: bool,
    pub token_estimate: usize,
    pub start_line: i64,
    pub end_line: i64,
}

/// Token-budgeted context capsule returned by `ContextEngine::get_capsule`.
#[derive(Debug, Clone, Serialize)]
pub struct ContextCapsule {
    pub intent: String,
    pub items: Vec<CapsuleItem>,
    pub memories: Vec<Memory>,
    pub total_tokens: usize,
    pub budget: usize,
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// All intent keywords across every category. Used to strip noise from the FTS
/// query so that words like "fix", "refactor", "add" don't pollute symbol search.
const ALL_INTENT_KEYWORDS: &[&str] = &[
    "fix", "bug", "crash", "fail", "panic", "broken", "debug",
    "refactor", "rename", "extract", "split", "reorganize",
    "add", "implement", "create", "build", "feature",
];

/// Strip intent keywords from a query, returning only the code-relevant terms
/// for FTS5 search. If stripping leaves nothing, returns the original query
/// to avoid an empty FTS match.
fn strip_intent_keywords(query: &str) -> String {
    let words: Vec<&str> = query
        .split_whitespace()
        .filter(|w| {
            let lower = w.to_lowercase();
            !ALL_INTENT_KEYWORDS.contains(&lower.as_str())
        })
        .collect();

    if words.is_empty() {
        // All words were intent keywords — fall back to original to avoid empty FTS
        query.to_string()
    } else {
        words.join(" ")
    }
}

/// Rough token estimate: ~4 chars per token. Good enough for budgeting without
/// pulling in a tokenizer dependency.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate tokens for a fully-rendered capsule item (name + kind + sig + body
/// + file path + line numbers). Mirrors what the serialized JSON will cost.
fn item_token_cost(sym: &Symbol, file_path: &str, include_body: bool) -> usize {
    let mut chars = sym.name.len() + sym.kind.len() + sym.signature.len() + file_path.len();
    // line number formatting overhead — small but accounted for
    chars += 20;
    if include_body {
        chars += sym.body.len();
    }
    chars.div_ceil(4)
}

// ---------------------------------------------------------------------------
// ContextEngine
// ---------------------------------------------------------------------------

pub struct ContextEngine<'a> {
    db: &'a Database,
}

impl<'a> ContextEngine<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Build a token-budgeted context capsule for `query`.
    ///
    /// Algorithm:
    /// 1. Detect intent from query text.
    /// 2. Phase 1 — FTS5 search for pivot symbols (top 5), add with full body.
    /// 3. Phase 2 — Expand to adjacent symbols via the dependency graph,
    ///    direction driven by intent. Adjacent symbols get skeleton only.
    /// 4. Phase 3 — Attach memories linked to pivot symbols, capped at 10%
    ///    of the token budget.
    /// 5. Respect token budget at every step; stop adding when exhausted.
    pub fn get_capsule(
        &self,
        query: &str,
        max_tokens: usize,
        repo_id: Option<i64>,
        already_sent: &HashSet<i64>,
    ) -> anyhow::Result<ContextCapsule> {
        let intent = Intent::detect(query);
        let budget = max_tokens;
        let mut used_tokens: usize = 0;
        let mut items: Vec<CapsuleItem> = Vec::new();
        let mut seen_ids: HashSet<i64> = HashSet::new();

        // ----- Phase 1: Pivot symbols via FTS5 (top 5) -----
        // Strip intent keywords ("fix", "refactor", etc.) so they don't pollute
        // the FTS5 match. The user is describing *what to do*, not *what to find*.
        let fts_query = strip_intent_keywords(query);

        // Apply recency bias for debug intent: recently-changed files are more
        // likely to contain the bug. Other intents get pure BM25 ranking.
        let recency_boost = match intent {
            Intent::Debug => 0.5,
            _ => 0.0,
        };
        let mut pivots = self
            .db
            .search_code_with_recency(&fts_query, "", repo_id, 5, recency_boost)?;

        // Fallback: if FTS returned < 3 results, try fuzzy name match.
        // FTS5 tokenizes on whitespace/punctuation and misses camelCase
        // symbol names or partial matches that LIKE can catch.
        if pivots.len() < 3 {
            let terms: Vec<&str> = fts_query.split_whitespace().collect();
            if let Ok(fallback) = self.db.search_symbols_by_name_like(&terms, repo_id, 5) {
                for sym in fallback {
                    if pivots.len() >= 5 {
                        break;
                    }
                    // Avoid duplicates — seen_ids isn't populated yet, check pivots directly
                    if !pivots.iter().any(|p| p.id == sym.id) {
                        pivots.push(sym);
                    }
                }
            }
        }

        for sym in &pivots {
            let file_path = self
                .db
                .get_file_path_for_symbol(sym.id)
                .unwrap_or_else(|_| "<unknown>".to_string());

            let include_body = !already_sent.contains(&sym.id);
            let cost = item_token_cost(sym, &file_path, include_body);
            if used_tokens + cost > budget {
                break;
            }

            items.push(CapsuleItem {
                symbol_id: sym.id,
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                file_path,
                signature: sym.signature.clone(),
                body: if include_body {
                    sym.body.clone()
                } else {
                    "(full body sent earlier in session)".to_string()
                },
                is_pivot: true,
                token_estimate: cost,
                start_line: sym.start_line,
                end_line: sym.end_line,
            });
            used_tokens += cost;
            seen_ids.insert(sym.id);
        }

        // ----- Phase 2: Expand to adjacent symbols -----
        // Collect adjacent symbols from graph edges, driven by intent.
        let mut adjacent_symbols: Vec<(Symbol, String)> = Vec::new();

        for pivot in &pivots {
            if !seen_ids.contains(&pivot.id) {
                // pivot was skipped due to budget — don't expand from it
                continue;
            }

            match intent {
                Intent::Debug => {
                    // Callers (dependents) + dependencies
                    if let Ok(dependents) = self.db.get_dependents(pivot.id) {
                        for (_edge, sym) in dependents {
                            if seen_ids.insert(sym.id) {
                                let fp = self
                                    .db
                                    .get_file_path_for_symbol(sym.id)
                                    .unwrap_or_else(|_| "<unknown>".to_string());
                                adjacent_symbols.push((sym, fp));
                            }
                        }
                    }
                    if let Ok(deps) = self.db.get_dependencies(pivot.id) {
                        for (_edge, sym) in deps {
                            if seen_ids.insert(sym.id) {
                                let fp = self
                                    .db
                                    .get_file_path_for_symbol(sym.id)
                                    .unwrap_or_else(|_| "<unknown>".to_string());
                                adjacent_symbols.push((sym, fp));
                            }
                        }
                    }
                }
                Intent::Refactor => {
                    // Blast radius: dependents only
                    if let Ok(dependents) = self.db.get_dependents(pivot.id) {
                        for (_edge, sym) in dependents {
                            if seen_ids.insert(sym.id) {
                                let fp = self
                                    .db
                                    .get_file_path_for_symbol(sym.id)
                                    .unwrap_or_else(|_| "<unknown>".to_string());
                                adjacent_symbols.push((sym, fp));
                            }
                        }
                    }
                }
                Intent::Modify | Intent::Explore => {
                    // Dependencies only
                    if let Ok(deps) = self.db.get_dependencies(pivot.id) {
                        for (_edge, sym) in deps {
                            if seen_ids.insert(sym.id) {
                                let fp = self
                                    .db
                                    .get_file_path_for_symbol(sym.id)
                                    .unwrap_or_else(|_| "<unknown>".to_string());
                                adjacent_symbols.push((sym, fp));
                            }
                        }
                    }
                }
            }
        }

        // Add adjacent symbols as skeletons (no body)
        for (sym, file_path) in &adjacent_symbols {
            let cost = item_token_cost(sym, file_path, false);
            if used_tokens + cost > budget {
                break;
            }

            items.push(CapsuleItem {
                symbol_id: sym.id,
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                file_path: file_path.clone(),
                signature: sym.signature.clone(),
                body: String::new(),
                is_pivot: false,
                token_estimate: cost,
                start_line: sym.start_line,
                end_line: sym.end_line,
            });
            used_tokens += cost;
        }

        // ----- Phase 3: Attach memories (up to 10% of budget) -----
        let memory_budget = budget / 10;
        let mut memory_tokens: usize = 0;
        let mut memories: Vec<Memory> = Vec::new();

        for pivot in &pivots {
            if memory_tokens >= memory_budget {
                break;
            }
            let mems = self
                .db
                .get_memories_for_symbol(pivot.id, false)
                .unwrap_or_default();
            for mem in mems {
                let cost = estimate_tokens(&mem.content);
                if memory_tokens + cost > memory_budget {
                    break;
                }
                memory_tokens += cost;
                memories.push(mem);
            }
        }
        used_tokens += memory_tokens;

        Ok(ContextCapsule {
            intent: format!("{:?}", intent).to_lowercase(),
            items,
            memories,
            total_tokens: used_tokens,
            budget,
        })
    }
}
