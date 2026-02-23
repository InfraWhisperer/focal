use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::db::{Database, Symbol};

// ---------------------------------------------------------------------------
// ImpactNode — one node in the blast-radius graph
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ImpactNode {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub distance: usize,
    pub edge_kind: String,
}

// ---------------------------------------------------------------------------
// GraphEngine — BFS-based graph traversal over the symbol dependency graph
// ---------------------------------------------------------------------------

pub struct GraphEngine<'a> {
    db: &'a Database,
}

impl<'a> GraphEngine<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// BFS traversal of reverse edges (dependents) to find the blast radius
    /// of changing `symbol_name`. Returns all symbols transitively affected,
    /// up to `max_depth` hops away.
    pub fn impact_graph(
        &self,
        symbol_name: &str,
        max_depth: usize,
        repo_id: Option<i64>,
    ) -> anyhow::Result<Vec<ImpactNode>> {
        let root = self.resolve_symbol(symbol_name, repo_id)?;

        let mut visited = HashSet::new();
        visited.insert(root.id);

        let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
        queue.push_back((root.id, 0));

        let mut results = Vec::new();

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            // Reverse edges: who depends on current_id?
            let dependents = self.db.get_dependents(current_id)?;

            for (edge, sym) in dependents {
                if visited.insert(sym.id) {
                    let file_path = self
                        .db
                        .get_file_path_for_symbol(sym.id)
                        .unwrap_or_else(|_| "<unknown>".to_string());

                    results.push(ImpactNode {
                        name: sym.name.clone(),
                        kind: sym.kind.clone(),
                        file_path,
                        distance: depth + 1,
                        edge_kind: edge.kind.clone(),
                    });

                    queue.push_back((sym.id, depth + 1));
                }
            }
        }

        Ok(results)
    }

    /// BFS pathfinding through forward edges (dependencies) from `from_name`
    /// to `to_name`. Returns up to `max_paths` distinct paths, each capped
    /// at length 10 to prevent runaway traversal.
    pub fn find_paths(
        &self,
        from_name: &str,
        to_name: &str,
        max_paths: usize,
        repo_id: Option<i64>,
    ) -> anyhow::Result<Vec<Vec<Symbol>>> {
        let source = self.resolve_symbol(from_name, repo_id)?;
        let target = self.resolve_symbol(to_name, repo_id)?;

        const MAX_PATH_LEN: usize = 10;
        const MAX_QUEUE_SIZE: usize = 10_000;

        let mut found_paths: Vec<Vec<i64>> = Vec::new();
        let mut queue: VecDeque<Vec<i64>> = VecDeque::new();
        queue.push_back(vec![source.id]);

        // Cache symbol data for reconstruction at the end
        let mut symbol_cache: HashMap<i64, Symbol> = HashMap::new();
        let target_id = target.id;
        symbol_cache.insert(source.id, source);
        symbol_cache.insert(target_id, target);

        while let Some(path) = queue.pop_front() {
            if found_paths.len() >= max_paths || queue.len() > MAX_QUEUE_SIZE {
                break;
            }

            let current_id = *path.last().unwrap();

            if current_id == target_id {
                found_paths.push(path);
                continue;
            }

            if path.len() >= MAX_PATH_LEN {
                continue;
            }

            let visited_on_path: HashSet<i64> = path.iter().copied().collect();
            let deps = self.db.get_dependencies(current_id)?;

            for (_edge, dep_sym) in deps {
                if !visited_on_path.contains(&dep_sym.id) {
                    symbol_cache.entry(dep_sym.id).or_insert_with(|| dep_sym.clone());
                    let mut new_path = path.clone();
                    new_path.push(dep_sym.id);
                    queue.push_back(new_path);
                }
            }
        }

        // Reconstruct symbol paths from ID paths
        Ok(found_paths
            .into_iter()
            .map(|id_path| {
                id_path
                    .into_iter()
                    .filter_map(|id| symbol_cache.get(&id).cloned())
                    .collect()
            })
            .collect())
    }

    /// Resolve a symbol name to a `Symbol`, optionally scoped to a repo.
    fn resolve_symbol(
        &self,
        name: &str,
        repo_id: Option<i64>,
    ) -> anyhow::Result<Symbol> {
        let sym = match repo_id {
            Some(rid) => self.db.find_symbol_by_name(rid, name)?,
            None => self.db.find_symbol_by_name_any(name)?,
        };
        sym.ok_or_else(|| anyhow::anyhow!("symbol '{}' not found", name))
    }
}
