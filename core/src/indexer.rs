use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::db::Database;
use crate::grammar::{ExtractedSymbol, GrammarRegistry};

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub symbols_extracted: usize,
    pub edges_created: usize,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Indexer
// ---------------------------------------------------------------------------

pub struct Indexer<'a> {
    db: &'a Database,
    registry: &'a GrammarRegistry,
    exclude_patterns: HashSet<String>,
    max_file_size: u64,
}

impl<'a> Indexer<'a> {
    pub fn new(db: &'a Database, registry: &'a GrammarRegistry) -> Self {
        Self {
            db,
            registry,
            exclude_patterns: HashSet::from([
                "node_modules".to_string(),
                ".git".to_string(),
                "vendor".to_string(),
                "target".to_string(),
                "dist".to_string(),
                "__pycache__".to_string(),
            ]),
            max_file_size: 500 * 1024, // 500 KB
        }
    }

    pub fn with_excludes(mut self, patterns: Vec<String>) -> Self {
        self.exclude_patterns = patterns.into_iter().collect();
        self
    }

    pub fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    /// Main entry point: walk a directory, parse supported files, store symbols,
    /// then resolve cross-file call edges.
    pub fn index_directory(&self, root: &Path) -> Result<IndexStats> {
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", root.display()))?;

        let repo_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());

        let root_str = root.to_string_lossy().to_string();
        let repo_id = self.db.upsert_repository(&repo_name, &root_str)?;

        self.db.with_transaction(|| {
            let mut stats = IndexStats::default();

            // Phase 1: walk files, parse symbols, store in DB
            for entry in WalkDir::new(&root)
                .into_iter()
                .filter_entry(|e| !self.is_excluded(e.path()))
            {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        stats.errors.push(format!("walk error: {err}"));
                        continue;
                    }
                };

                if !entry.file_type().is_file() {
                    continue;
                }

                let path = entry.path();

                // Check grammar support by extension
                let ext = match path.extension().and_then(|e| e.to_str()) {
                    Some(e) => e,
                    None => continue,
                };
                let grammar = match self.registry.for_extension(ext) {
                    Some(g) => g,
                    None => continue,
                };

                // Check file size
                let metadata = match std::fs::metadata(path) {
                    Ok(m) => m,
                    Err(err) => {
                        stats.errors.push(format!("{}: metadata error: {err}", path.display()));
                        continue;
                    }
                };
                if metadata.len() > self.max_file_size {
                    stats.files_skipped += 1;
                    continue;
                }

                // Read file
                let source = match std::fs::read(path) {
                    Ok(s) => s,
                    Err(err) => {
                        stats.errors.push(format!("{}: read error: {err}", path.display()));
                        continue;
                    }
                };

                // Compute SHA-256
                let hash = {
                    let mut hasher = Sha256::new();
                    hasher.update(&source);
                    format!("{:x}", hasher.finalize())
                };

                // Relative path within repo
                let rel_path = path
                    .strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                // Skip if hash unchanged
                if let Some(existing_hash) = self.db.get_file_hash(repo_id, &rel_path)? {
                    if existing_hash == hash {
                        stats.files_skipped += 1;
                        continue;
                    }
                }

                // Detect language name
                let language = self
                    .registry
                    .detect_language(path)
                    .unwrap_or(ext);

                // Upsert file record
                let file_id = self.db.upsert_file(repo_id, &rel_path, language, &hash)?;

                // Mark linked memories stale (file was re-indexed)
                let _ = self.db.mark_memories_stale_for_file(file_id);

                // Snapshot memory->symbol_name links before deletion so we can
                // re-link to the new symbol IDs after re-insertion.
                let memory_links = self
                    .db
                    .collect_memory_symbol_names(file_id)
                    .unwrap_or_default();

                // Clear old symbols (and edges referencing them)
                let _ = self.db.delete_edges_by_file(file_id);
                let _ = self.db.delete_symbols_by_file(file_id);

                // Parse with tree-sitter
                let mut parser = tree_sitter::Parser::new();
                let ts_lang = grammar.language();
                if let Err(err) = parser.set_language(&ts_lang) {
                    stats.errors.push(format!("{}: set_language error: {err}", path.display()));
                    continue;
                }

                let tree = match parser.parse(&source, None) {
                    Some(t) => t,
                    None => {
                        stats.errors.push(format!("{}: parse returned None", path.display()));
                        continue;
                    }
                };

                // Extract and insert symbols
                let symbols = grammar.extract_symbols(&source, &tree);
                let inserted = self.insert_symbols_recursive(file_id, &symbols, None)?;
                stats.symbols_extracted += inserted;
                stats.files_indexed += 1;

                // Re-link memories to new symbols by matching names
                if !memory_links.is_empty() {
                    let _ = self.db.relink_memories_to_symbols(file_id, &memory_links);
                }
            }

            // Phase 2: resolve cross-file edges
            let edge_count = self.resolve_edges(repo_id, &root)?;
            stats.edges_created = edge_count;

            Ok(stats)
        })
    }

    /// Re-index a single file. Determines the repo from the path, checks hash,
    /// and updates symbols + edges if changed. Returns true if re-indexed.
    pub fn index_file(&self, file_path: &Path, root: &Path) -> Result<bool> {
        let root = root.canonicalize()?;
        let repo_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let root_str = root.to_string_lossy().to_string();
        let repo_id = self.db.upsert_repository(&repo_name, &root_str)?;

        let ext = match file_path.extension().and_then(|e| e.to_str()) {
            Some(e) => e,
            None => return Ok(false),
        };
        let grammar = match self.registry.for_extension(ext) {
            Some(g) => g,
            None => return Ok(false),
        };

        let source = std::fs::read(file_path)?;
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&source);
            format!("{:x}", hasher.finalize())
        };

        let rel_path = file_path
            .strip_prefix(&root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        if let Some(existing_hash) = self.db.get_file_hash(repo_id, &rel_path)? {
            if existing_hash == hash {
                return Ok(false); // unchanged
            }
        }

        let language = self.registry.detect_language(file_path).unwrap_or(ext);

        // Parse outside the transaction — this is pure computation
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&grammar.language())?;
        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("parse returned None"))?;
        let symbols = grammar.extract_symbols(&source, &tree);
        let refs = grammar.extract_references(&source, &tree);

        // All DB mutations wrapped in a transaction for atomicity
        self.db.with_transaction(|| {
            let file_id = self.db.upsert_file(repo_id, &rel_path, language, &hash)?;
            let _ = self.db.mark_memories_stale_for_file(file_id);
            let memory_links = self
                .db
                .collect_memory_symbol_names(file_id)
                .unwrap_or_default();
            let _ = self.db.delete_edges_by_file(file_id);
            let _ = self.db.delete_symbols_by_file(file_id);

            self.insert_symbols_recursive(file_id, &symbols, None)?;

            if !memory_links.is_empty() {
                let _ = self.db.relink_memories_to_symbols(file_id, &memory_links);
            }

            // Re-resolve edges for this file using the repo-wide symbol map
            let symbol_map = self.db.get_all_symbol_names_for_repo(repo_id)?;
            let file_symbols = self.db.get_symbols_by_file(file_id)?;
            for r in &refs {
                let source_sym = file_symbols.iter().find(|s| s.name == r.from_symbol);
                let target_id = symbol_map.get(&r.to_name);
                if let (Some(src), Some(&tgt_id)) = (source_sym, target_id) {
                    if src.id != tgt_id {
                        let _ = self.db.insert_edge(src.id, tgt_id, &r.kind);
                    }
                }
            }

            Ok(true)
        })
    }

    /// Remove a deleted file's symbols and edges from the index.
    /// Returns true if the file was found and removed.
    pub fn remove_deleted_file(&self, file_path: &Path, root: &Path) -> Result<bool> {
        let root = root.canonicalize()?;
        let repo_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let root_str = root.to_string_lossy().to_string();
        let repo_id = self.db.upsert_repository(&repo_name, &root_str)?;

        let rel_path = file_path
            .strip_prefix(&root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        self.db.remove_file(repo_id, &rel_path)
    }

    /// Recursively insert extracted symbols and their children. Returns the count inserted.
    /// Computes a SHA-256 hash of each symbol's body for content-aware memory staleness.
    fn insert_symbols_recursive(
        &self,
        file_id: i64,
        symbols: &[ExtractedSymbol],
        parent_id: Option<i64>,
    ) -> Result<usize> {
        let mut count = 0;
        for sym in symbols {
            let body_hash = {
                let mut hasher = Sha256::new();
                hasher.update(sym.body.as_bytes());
                format!("{:x}", hasher.finalize())
            };
            let sym_id = self.db.insert_symbol(
                file_id,
                &sym.name,
                sym.kind.as_str(),
                &sym.signature,
                &sym.body,
                &body_hash,
                sym.start_line as i64,
                sym.end_line as i64,
                parent_id,
            )?;
            count += 1;
            count += self.insert_symbols_recursive(file_id, &sym.children, Some(sym_id))?;
        }
        Ok(count)
    }

    /// For each file in the repo, re-parse and extract references, then resolve
    /// each reference against the symbol table to create edges.
    ///
    /// Uses a pre-built name->id HashMap instead of per-reference SQL lookups.
    /// This turns O(refs * query_cost) into O(refs) with a single up-front query.
    fn resolve_edges(&self, repo_id: i64, root: &Path) -> Result<usize> {
        // Build name→id map once for the whole repo
        let symbol_map = self.db.get_all_symbol_names_for_repo(repo_id)?;
        let files = self.db.get_files_for_repo(repo_id)?;
        let mut edge_count = 0;

        for file_record in &files {
            let abs_path = root.join(&file_record.path);

            let ext = match PathBuf::from(&file_record.path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_string())
            {
                Some(e) => e,
                None => continue,
            };

            let grammar = match self.registry.for_extension(&ext) {
                Some(g) => g,
                None => continue,
            };

            let source = match std::fs::read(&abs_path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let mut parser = tree_sitter::Parser::new();
            let ts_lang = grammar.language();
            if parser.set_language(&ts_lang).is_err() {
                continue;
            }
            let tree = match parser.parse(&source, None) {
                Some(t) => t,
                None => continue,
            };

            let refs = grammar.extract_references(&source, &tree);
            let file_symbols = self.db.get_symbols_by_file(file_record.id)?;

            for r in &refs {
                let source_sym = file_symbols.iter().find(|s| s.name == r.from_symbol);
                let target_id = symbol_map.get(&r.to_name);

                if let (Some(src), Some(&tgt_id)) = (source_sym, target_id) {
                    if src.id != tgt_id {
                        self.db.insert_edge(src.id, tgt_id, &r.kind)?;
                        edge_count += 1;
                    }
                }
            }
        }

        Ok(edge_count)
    }

    /// Returns true if any component of the path matches an exclude pattern.
    fn is_excluded(&self, path: &Path) -> bool {
        for component in path.components() {
            if self.exclude_patterns.contains(component.as_os_str().to_string_lossy().as_ref()) {
                return true;
            }
        }
        false
    }
}
