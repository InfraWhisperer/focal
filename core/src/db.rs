use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Repository {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub indexed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub language: String,
    pub hash: String,
    pub indexed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub id: i64,
    pub file_id: i64,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub body: String,
    pub body_hash: String,
    pub start_line: i64,
    pub end_line: i64,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: String,
    pub session_id: String,
    pub created_at: String,
    pub stale: bool,
    /// Set when a linked symbol's body changed but its name still matches.
    /// The memory may still be valid but should be verified against the new code.
    pub needs_review: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolResult {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub body: String,
    pub file_path: String,
    pub repo_name: String,
    pub start_line: i64,
    pub end_line: i64,
    pub memories: Vec<Memory>,
    /// Hints about unseen dependencies (e.g. "Implements trait Foo (not in context)")
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependency_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolSummary {
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub start_line: i64,
    pub end_line: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub db_size_bytes: i64,
    pub symbol_count: i64,
    pub file_count: i64,
    pub edge_count: i64,
    pub memory_count: i64,
    pub repo_count: i64,
    pub fts_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecoveryData {
    pub session_id: String,
    pub manual_memories: Vec<Memory>,
    pub auto_observations: Vec<Memory>,
    pub recent_files: Vec<String>,
    pub symbol_names_accessed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoOverview {
    pub name: String,
    pub root_path: String,
    pub file_count: i64,
    pub symbol_count: i64,
    pub memory_count: i64,
    pub languages: Vec<LanguageCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LanguageCount {
    pub language: String,
    pub count: i64,
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a SQLite database at `path` and run migrations.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open database at {path}"))?;
        let db = Self { conn };
        db.apply_pragmas()?;
        db.migrate()?;
        Ok(db)
    }

    /// In-memory database for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("failed to open in-memory database")?;
        let db = Self { conn };
        db.apply_pragmas()?;
        db.migrate()?;
        Ok(db)
    }

    fn apply_pragmas(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    /// Execute `f` inside an IMMEDIATE transaction. Commits on Ok, rolls back on Err.
    pub fn with_transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f() {
            Ok(val) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(val)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS repositories (
                id         INTEGER PRIMARY KEY,
                name       TEXT NOT NULL,
                root_path  TEXT NOT NULL UNIQUE,
                indexed_at TEXT
            );

            CREATE TABLE IF NOT EXISTS files (
                id         INTEGER PRIMARY KEY,
                repo_id    INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                path       TEXT NOT NULL,
                language   TEXT NOT NULL,
                hash       TEXT NOT NULL,
                indexed_at TEXT,
                UNIQUE(repo_id, path)
            );

            CREATE TABLE IF NOT EXISTS symbols (
                id         INTEGER PRIMARY KEY,
                file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                name       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                signature  TEXT NOT NULL DEFAULT '',
                body       TEXT NOT NULL DEFAULT '',
                body_hash  TEXT NOT NULL DEFAULT '',
                start_line INTEGER NOT NULL,
                end_line   INTEGER NOT NULL,
                parent_id  INTEGER REFERENCES symbols(id) ON DELETE SET NULL
            );

            CREATE TABLE IF NOT EXISTS edges (
                id        INTEGER PRIMARY KEY,
                source_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
                target_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
                kind      TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memories (
                id            INTEGER PRIMARY KEY,
                content       TEXT NOT NULL,
                category      TEXT NOT NULL,
                source        TEXT NOT NULL DEFAULT 'manual',
                session_id    TEXT NOT NULL DEFAULT '',
                created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                stale         INTEGER NOT NULL DEFAULT 0,
                needs_review  INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS memory_symbols (
                memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
                PRIMARY KEY (memory_id, symbol_id)
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_files_repo_id        ON files(repo_id);
            CREATE INDEX IF NOT EXISTS idx_symbols_file_name     ON symbols(file_id, name);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind_name     ON symbols(kind, name);
            CREATE INDEX IF NOT EXISTS idx_edges_source          ON edges(source_id);
            CREATE INDEX IF NOT EXISTS idx_edges_target          ON edges(target_id);
            CREATE INDEX IF NOT EXISTS idx_memory_symbols_sym    ON memory_symbols(symbol_id);

            -- Name-only index for find_symbol_by_name_any
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);

            -- Prevent duplicate edges
            CREATE UNIQUE INDEX IF NOT EXISTS idx_edges_unique
                ON edges(source_id, target_id, kind);
            ",
        )?;

        // FTS5 virtual table — CREATE VIRTUAL TABLE … IF NOT EXISTS is supported
        // since SQLite 3.37.0, which rusqlite bundles well past that.
        self.conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts
             USING fts5(name, signature, body, content=symbols, content_rowid=id);",
        )?;

        self.conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts
             USING fts5(content, category, content=memories, content_rowid=id);",
        )?;

        // Additive migrations for existing databases. Each uses a conditional
        // check so they're idempotent — safe to run on every startup.
        self.apply_additive_migrations()?;

        Ok(())
    }

    /// Column additions for existing databases. Each migration checks whether
    /// the column already exists before altering the table, so this is safe
    /// to call on every startup.
    fn apply_additive_migrations(&self) -> Result<()> {
        // v0.2.0: body_hash on symbols for content-aware memory staleness
        let has_body_hash: bool = self
            .conn
            .prepare("SELECT body_hash FROM symbols LIMIT 0")
            .is_ok();
        if !has_body_hash {
            self.conn.execute_batch(
                "ALTER TABLE symbols ADD COLUMN body_hash TEXT NOT NULL DEFAULT '';"
            )?;
        }

        // v0.2.0: needs_review on memories for body-changed-but-name-same detection
        let has_needs_review: bool = self
            .conn
            .prepare("SELECT needs_review FROM memories LIMIT 0")
            .is_ok();
        if !has_needs_review {
            self.conn.execute_batch(
                "ALTER TABLE memories ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 0;"
            )?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Repository CRUD
    // -----------------------------------------------------------------------

    pub fn upsert_repository(&self, name: &str, root_path: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO repositories (name, root_path, indexed_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(root_path) DO UPDATE SET name = excluded.name,
                                                  indexed_at = excluded.indexed_at",
            params![name, root_path],
        )?;
        // ON CONFLICT UPDATE leaves last_insert_rowid stale (reflects the last
        // real INSERT on this connection, not this statement). Always SELECT to
        // get the authoritative id.
        let id: i64 = self.conn.query_row(
            "SELECT id FROM repositories WHERE root_path = ?1",
            params![root_path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn get_repository_by_path(&self, root_path: &str) -> Result<Option<Repository>> {
        let r = self
            .conn
            .query_row(
                "SELECT id, name, root_path, indexed_at FROM repositories WHERE root_path = ?1",
                params![root_path],
                |row| {
                    Ok(Repository {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        root_path: row.get(2)?,
                        indexed_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(r)
    }

    pub fn get_repo_id_by_name(&self, name: &str) -> Result<Option<i64>> {
        let r = self
            .conn
            .query_row(
                "SELECT id FROM repositories WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(r)
    }

    // -----------------------------------------------------------------------
    // File CRUD
    // -----------------------------------------------------------------------

    pub fn upsert_file(
        &self,
        repo_id: i64,
        path: &str,
        language: &str,
        hash: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO files (repo_id, path, language, hash, indexed_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(repo_id, path) DO UPDATE SET language   = excluded.language,
                                                      hash       = excluded.hash,
                                                      indexed_at = excluded.indexed_at",
            params![repo_id, path, language, hash],
        )?;
        // Always SELECT — last_insert_rowid is unreliable on the UPDATE path
        // of an ON CONFLICT upsert.
        let id: i64 = self.conn.query_row(
            "SELECT id FROM files WHERE repo_id = ?1 AND path = ?2",
            params![repo_id, path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn get_file_by_path(&self, repo_id: i64, path: &str) -> Result<Option<FileRecord>> {
        let r = self
            .conn
            .query_row(
                "SELECT id, repo_id, path, language, hash, indexed_at
                 FROM files WHERE repo_id = ?1 AND path = ?2",
                params![repo_id, path],
                |row| {
                    Ok(FileRecord {
                        id: row.get(0)?,
                        repo_id: row.get(1)?,
                        path: row.get(2)?,
                        language: row.get(3)?,
                        hash: row.get(4)?,
                        indexed_at: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(r)
    }

    /// Remove a file and all its symbols/edges from the index.
    /// Returns true if a file record was actually deleted.
    pub fn remove_file(&self, repo_id: i64, rel_path: &str) -> Result<bool> {
        let file_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM files WHERE repo_id = ?1 AND path = ?2",
                params![repo_id, rel_path],
                |row| row.get(0),
            )
            .optional()?;
        let file_id = match file_id {
            Some(id) => id,
            None => return Ok(false),
        };
        self.delete_edges_by_file(file_id)?;
        self.delete_symbols_by_file(file_id)?;
        self.conn
            .execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
        Ok(true)
    }

    pub fn get_file_hash(&self, repo_id: i64, path: &str) -> Result<Option<String>> {
        let r = self
            .conn
            .query_row(
                "SELECT hash FROM files WHERE repo_id = ?1 AND path = ?2",
                params![repo_id, path],
                |row| row.get(0),
            )
            .optional()?;
        Ok(r)
    }

    pub fn get_files_for_repo(&self, repo_id: i64) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, path, language, hash, indexed_at
             FROM files WHERE repo_id = ?1 ORDER BY path",
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                repo_id: row.get(1)?,
                path: row.get(2)?,
                language: row.get(3)?,
                hash: row.get(4)?,
                indexed_at: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_file_path_for_symbol(&self, symbol_id: i64) -> Result<String> {
        let path: String = self.conn.query_row(
            "SELECT f.path FROM files f
             JOIN symbols s ON s.file_id = f.id
             WHERE s.id = ?1",
            params![symbol_id],
            |row| row.get(0),
        )?;
        Ok(path)
    }

    // -----------------------------------------------------------------------
    // Symbol CRUD
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn insert_symbol(
        &self,
        file_id: i64,
        name: &str,
        kind: &str,
        signature: &str,
        body: &str,
        body_hash: &str,
        start_line: i64,
        end_line: i64,
        parent_id: Option<i64>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO symbols (file_id, name, kind, signature, body, body_hash, start_line, end_line, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![file_id, name, kind, signature, body, body_hash, start_line, end_line, parent_id],
        )?;
        let id = self.conn.last_insert_rowid();
        // Maintain FTS index incrementally
        self.conn.execute(
            "INSERT INTO symbols_fts(rowid, name, signature, body) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, signature, body],
        )?;
        Ok(id)
    }

    pub fn get_symbols_by_file(&self, file_id: i64) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_id, name, kind, signature, body, body_hash, start_line, end_line, parent_id
             FROM symbols WHERE file_id = ?1 ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![file_id], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                signature: row.get(4)?,
                body: row.get(5)?,
                body_hash: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                parent_id: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_symbols_by_file(&self, file_id: i64) -> Result<usize> {
        // Remove from FTS index before deleting the content rows
        self.conn.execute(
            "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id],
        )?;
        let count = self
            .conn
            .execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])?;
        Ok(count)
    }

    pub fn find_symbol_by_name(&self, repo_id: i64, name: &str) -> Result<Option<Symbol>> {
        let r = self
            .conn
            .query_row(
                "SELECT s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                        s.body_hash, s.start_line, s.end_line, s.parent_id
                 FROM symbols s
                 JOIN files f ON f.id = s.file_id
                 WHERE f.repo_id = ?1 AND s.name = ?2
                 LIMIT 1",
                params![repo_id, name],
                |row| {
                    Ok(Symbol {
                        id: row.get(0)?,
                        file_id: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        signature: row.get(4)?,
                        body: row.get(5)?,
                        body_hash: row.get(6)?,
                        start_line: row.get(7)?,
                        end_line: row.get(8)?,
                        parent_id: row.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(r)
    }

    pub fn find_symbol_by_name_any(&self, name: &str) -> Result<Option<Symbol>> {
        let r = self
            .conn
            .query_row(
                "SELECT id, file_id, name, kind, signature, body,
                        body_hash, start_line, end_line, parent_id
                 FROM symbols WHERE name = ?1 ORDER BY id LIMIT 1",
                params![name],
                |row| {
                    Ok(Symbol {
                        id: row.get(0)?,
                        file_id: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        signature: row.get(4)?,
                        body: row.get(5)?,
                        body_hash: row.get(6)?,
                        start_line: row.get(7)?,
                        end_line: row.get(8)?,
                        parent_id: row.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(r)
    }

    /// Load all symbols in a repo as a HashMap keyed by name.
    /// For ambiguous names, prefers functions/methods over types.
    pub fn get_all_symbol_names_for_repo(
        &self,
        repo_id: i64,
    ) -> Result<std::collections::HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.name, s.kind FROM symbols s
             JOIN files f ON f.id = s.file_id
             WHERE f.repo_id = ?1
             ORDER BY CASE s.kind
                WHEN 'function' THEN 0
                WHEN 'method' THEN 1
                ELSE 2
             END",
        )?;
        let rows = stmt.query_map(params![repo_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for r in rows {
            let (id, name) = r?;
            map.entry(name).or_insert(id); // first wins (function/method preferred)
        }
        // Add unqualified aliases for qualified names (e.g., "Config::new" → "new").
        // Only insert if no existing entry — standalone symbols take priority.
        let aliases: Vec<(String, i64)> = map
            .iter()
            .filter_map(|(name, &id)| {
                name.rfind("::").map(|pos| (name[pos + 2..].to_string(), id))
            })
            .collect();
        for (short_name, id) in aliases {
            map.entry(short_name).or_insert(id);
        }
        Ok(map)
    }

    /// Rich symbol query: returns symbols with file path, repo name, and linked memories.
    /// Filters are all optional — pass empty string or None to skip.
    pub fn query_symbols_full(
        &self,
        name: &str,
        kind: &str,
        repo_name: &str,
    ) -> Result<Vec<SymbolResult>> {
        let mut sql = String::from(
            "SELECT s.id, s.name, s.kind, s.signature, s.body, s.body_hash,
                    f.path, r.name, s.start_line, s.end_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             JOIN repositories r ON r.id = f.repo_id
             WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if !name.is_empty() {
            sql.push_str(&format!(" AND s.name LIKE ?{idx}"));
            param_values.push(Box::new(format!("%{name}%")));
            idx += 1;
        }
        if !kind.is_empty() {
            sql.push_str(&format!(" AND s.kind = ?{idx}"));
            param_values.push(Box::new(kind.to_string()));
            idx += 1;
        }
        if !repo_name.is_empty() {
            sql.push_str(&format!(" AND r.name = ?{idx}"));
            param_values.push(Box::new(repo_name.to_string()));
            let _ = idx; // suppress unused warning
        }

        sql.push_str(" ORDER BY s.name LIMIT 200");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(SymbolResult {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                signature: row.get(3)?,
                body: row.get(4)?,
                file_path: row.get(6)?,
                repo_name: row.get(7)?,
                start_line: row.get(8)?,
                end_line: row.get(9)?,
                memories: Vec::new(), // filled below
                dependency_hints: Vec::new(), // filled later if requested
            })
        })?;

        let mut results: Vec<SymbolResult> = Vec::new();
        for r in rows {
            results.push(r?);
        }

        // Batch-load memories for all symbols in one query (avoids N+1)
        let sym_ids: Vec<i64> = results.iter().map(|s| s.id).collect();
        let mem_map = self.get_memories_for_symbols_batch(&sym_ids, false)?;
        for sym in &mut results {
            sym.memories = mem_map.get(&sym.id).cloned().unwrap_or_default();
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Edge CRUD
    // -----------------------------------------------------------------------

    pub fn insert_edge(&self, source_id: i64, target_id: i64, kind: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (source_id, target_id, kind) VALUES (?1, ?2, ?3)",
            params![source_id, target_id, kind],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Outgoing edges: symbols that `symbol_id` depends on.
    pub fn get_dependencies(&self, symbol_id: i64) -> Result<Vec<(Edge, Symbol)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.source_id, e.target_id, e.kind,
                    s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                    s.body_hash, s.start_line, s.end_line, s.parent_id
             FROM edges e
             JOIN symbols s ON s.id = e.target_id
             WHERE e.source_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok((
                Edge {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    target_id: row.get(2)?,
                    kind: row.get(3)?,
                },
                Symbol {
                    id: row.get(4)?,
                    file_id: row.get(5)?,
                    name: row.get(6)?,
                    kind: row.get(7)?,
                    signature: row.get(8)?,
                    body: row.get(9)?,
                    body_hash: row.get(10)?,
                    start_line: row.get(11)?,
                    end_line: row.get(12)?,
                    parent_id: row.get(13)?,
                },
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Incoming edges: symbols that depend on `symbol_id`.
    pub fn get_dependents(&self, symbol_id: i64) -> Result<Vec<(Edge, Symbol)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.source_id, e.target_id, e.kind,
                    s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                    s.body_hash, s.start_line, s.end_line, s.parent_id
             FROM edges e
             JOIN symbols s ON s.id = e.source_id
             WHERE e.target_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok((
                Edge {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    target_id: row.get(2)?,
                    kind: row.get(3)?,
                },
                Symbol {
                    id: row.get(4)?,
                    file_id: row.get(5)?,
                    name: row.get(6)?,
                    kind: row.get(7)?,
                    signature: row.get(8)?,
                    body: row.get(9)?,
                    body_hash: row.get(10)?,
                    start_line: row.get(11)?,
                    end_line: row.get(12)?,
                    parent_id: row.get(13)?,
                },
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Return dependency hints for a symbol: names and kinds of symbols it
    /// depends on via type_ref or imports edges. Used to warn the LLM about
    /// interfaces/traits not included in the current context.
    pub fn get_dependency_hint_names(
        &self,
        symbol_id: i64,
        _exclude_ids: &std::collections::HashSet<i64>,
    ) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.name, s.kind, e.kind
             FROM edges e
             JOIN symbols s ON s.id = e.target_id
             WHERE e.source_id = ?1
               AND e.kind IN ('type_ref', 'imports', 'calls')",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut hints = Vec::new();
        for r in rows {
            let (name, kind, edge_kind) = r?;
            // Only hint about symbols not already in the result set
            // We can't check by ID here (we have names), so caller filters by ID
            hints.push((name, kind, edge_kind));
        }
        Ok(hints)
    }

    pub fn delete_edges_by_file(&self, file_id: i64) -> Result<usize> {
        let c1 = self.conn.execute(
            "DELETE FROM edges WHERE source_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id],
        )?;
        let c2 = self.conn.execute(
            "DELETE FROM edges WHERE target_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id],
        )?;
        Ok(c1 + c2)
    }

    // -----------------------------------------------------------------------
    // Memory CRUD
    // -----------------------------------------------------------------------

    pub fn save_memory(
        &self,
        content: &str,
        category: &str,
        symbol_ids: &[i64],
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO memories (content, category, source, session_id)
             VALUES (?1, ?2, 'manual', '')",
            params![content, category],
        )?;
        let memory_id = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO memories_fts(rowid, content, category) VALUES (?1, ?2, ?3)",
            params![memory_id, content, category],
        )?;
        self.link_memory_symbols(memory_id, symbol_ids)?;
        Ok(memory_id)
    }

    pub fn save_auto_observation(
        &self,
        content: &str,
        source: &str,
        session_id: &str,
        symbol_ids: &[i64],
    ) -> Result<i64> {
        // Dedup: if an observation from the same source in this session exists, update it
        let existing: Option<i64> = self.conn.query_row(
            "SELECT id FROM memories WHERE source = ?1 AND session_id = ?2 AND category = 'observation'
             ORDER BY created_at DESC LIMIT 1",
            params![source, session_id],
            |row| row.get(0),
        ).optional()?;

        let memory_id = if let Some(id) = existing {
            // Update existing observation content and timestamp
            self.conn.execute(
                "UPDATE memories SET content = ?1, created_at = datetime('now') WHERE id = ?2",
                params![content, id],
            )?;
            // Refresh FTS index
            self.conn.execute(
                "DELETE FROM memories_fts WHERE rowid = ?1",
                params![id],
            )?;
            self.conn.execute(
                "INSERT INTO memories_fts(rowid, content, category) VALUES (?1, ?2, 'observation')",
                params![id, content],
            )?;
            id
        } else {
            // Insert new observation
            self.conn.execute(
                "INSERT INTO memories (content, category, source, session_id)
                 VALUES (?1, 'observation', ?2, ?3)",
                params![content, source, session_id],
            )?;
            let id = self.conn.last_insert_rowid();
            self.conn.execute(
                "INSERT INTO memories_fts(rowid, content, category) VALUES (?1, ?2, 'observation')",
                params![id, content],
            )?;
            id
        };

        // Re-link symbols (clear old links, add new ones)
        self.conn.execute(
            "DELETE FROM memory_symbols WHERE memory_id = ?1",
            params![memory_id],
        )?;
        self.link_memory_symbols(memory_id, symbol_ids)?;
        Ok(memory_id)
    }

    fn link_memory_symbols(&self, memory_id: i64, symbol_ids: &[i64]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO memory_symbols (memory_id, symbol_id) VALUES (?1, ?2)",
        )?;
        for &sid in symbol_ids {
            stmt.execute(params![memory_id, sid])?;
        }
        Ok(())
    }

    /// List memories, optionally filtering by category, staleness, and linked symbol name.
    pub fn list_memories(
        &self,
        category: &str,
        include_stale: bool,
        symbol_name: &str,
    ) -> Result<Vec<Memory>> {
        let mut sql = String::from(
            "SELECT DISTINCT m.id, m.content, m.category, m.source, m.session_id,
                    m.created_at, m.stale, m.needs_review
             FROM memories m",
        );

        if !symbol_name.is_empty() {
            sql.push_str(
                " JOIN memory_symbols ms ON ms.memory_id = m.id
                 JOIN symbols s ON s.id = ms.symbol_id",
            );
        }

        sql.push_str(" WHERE 1=1");

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if !category.is_empty() {
            sql.push_str(&format!(" AND m.category = ?{idx}"));
            param_values.push(Box::new(category.to_string()));
            idx += 1;
        }
        if !include_stale {
            sql.push_str(" AND m.stale = 0");
        }
        if !symbol_name.is_empty() {
            sql.push_str(&format!(" AND s.name = ?{idx}"));
            param_values.push(Box::new(symbol_name.to_string()));
            let _ = idx;
        }

        sql.push_str(" ORDER BY m.created_at DESC");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Memory {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                session_id: row.get(4)?,
                created_at: row.get(5)?,
                stale: row.get::<_, i64>(6)? != 0,
                needs_review: row.get::<_, i64>(7)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_memories_for_symbol(
        &self,
        symbol_id: i64,
        include_stale: bool,
    ) -> Result<Vec<Memory>> {
        let sql = if include_stale {
            "SELECT m.id, m.content, m.category, m.source, m.session_id,
                    m.created_at, m.stale, m.needs_review
             FROM memories m
             JOIN memory_symbols ms ON ms.memory_id = m.id
             WHERE ms.symbol_id = ?1
             ORDER BY m.created_at DESC"
        } else {
            "SELECT m.id, m.content, m.category, m.source, m.session_id,
                    m.created_at, m.stale, m.needs_review
             FROM memories m
             JOIN memory_symbols ms ON ms.memory_id = m.id
             WHERE ms.symbol_id = ?1 AND m.stale = 0
             ORDER BY m.created_at DESC"
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(Memory {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                session_id: row.get(4)?,
                created_at: row.get(5)?,
                stale: row.get::<_, i64>(6)? != 0,
                needs_review: row.get::<_, i64>(7)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Load memories for multiple symbol IDs in a single query.
    /// Returns a HashMap from symbol_id to Vec<Memory>.
    pub fn get_memories_for_symbols_batch(
        &self,
        symbol_ids: &[i64],
        include_stale: bool,
    ) -> Result<std::collections::HashMap<i64, Vec<Memory>>> {
        if symbol_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders: String = symbol_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let stale_filter = if include_stale { "" } else { " AND m.stale = 0" };
        let sql = format!(
            "SELECT ms.symbol_id, m.id, m.content, m.category, m.source, m.session_id,
                    m.created_at, m.stale, m.needs_review
             FROM memories m
             JOIN memory_symbols ms ON ms.memory_id = m.id
             WHERE ms.symbol_id IN ({placeholders}){stale_filter}
             ORDER BY m.created_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            symbol_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Memory {
                    id: row.get(1)?,
                    content: row.get(2)?,
                    category: row.get(3)?,
                    source: row.get(4)?,
                    session_id: row.get(5)?,
                    created_at: row.get(6)?,
                    stale: row.get::<_, i64>(7)? != 0,
                    needs_review: row.get::<_, i64>(8)? != 0,
                },
            ))
        })?;
        let mut map: std::collections::HashMap<i64, Vec<Memory>> = std::collections::HashMap::new();
        for r in rows {
            let (sym_id, mem) = r?;
            map.entry(sym_id).or_default().push(mem);
        }
        Ok(map)
    }

    pub fn get_memory_by_id(&self, memory_id: i64) -> Result<Option<Memory>> {
        let r = self
            .conn
            .query_row(
                "SELECT id, content, category, source, session_id, created_at, stale, needs_review
                 FROM memories WHERE id = ?1",
                params![memory_id],
                |row| {
                    Ok(Memory {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        source: row.get(3)?,
                        session_id: row.get(4)?,
                        created_at: row.get(5)?,
                        stale: row.get::<_, i64>(6)? != 0,
                        needs_review: row.get::<_, i64>(7)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(r)
    }

    pub fn get_symbol_ids_for_memory(&self, memory_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT symbol_id FROM memory_symbols WHERE memory_id = ?1",
        )?;
        let rows = stmt.query_map(params![memory_id], |row| row.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_memory(&self, memory_id: i64) -> Result<bool> {
        // Remove from FTS index before deleting the content row
        self.conn.execute(
            "DELETE FROM memories_fts WHERE rowid = ?1",
            params![memory_id],
        )?;
        // memory_symbols cascade-deletes via ON DELETE CASCADE
        let count = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![memory_id])?;
        Ok(count > 0)
    }

    pub fn update_memory(
        &self,
        memory_id: i64,
        content: &str,
        category: &str,
        symbol_ids: &[i64],
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET content = ?1, category = ?2 WHERE id = ?3",
            params![content, category, memory_id],
        )?;
        // Sync FTS index
        self.conn.execute(
            "DELETE FROM memories_fts WHERE rowid = ?1",
            params![memory_id],
        )?;
        self.conn.execute(
            "INSERT INTO memories_fts(rowid, content, category) VALUES (?1, ?2, ?3)",
            params![memory_id, content, category],
        )?;
        // Replace symbol links
        self.conn.execute(
            "DELETE FROM memory_symbols WHERE memory_id = ?1",
            params![memory_id],
        )?;
        self.link_memory_symbols(memory_id, symbol_ids)?;
        Ok(())
    }

    /// Collect (memory_id, symbol_name, body_hash) tuples for all memories linked
    /// to symbols in `file_id`. Used to re-link memories after re-indexing and
    /// detect body changes that warrant a needs_review flag.
    pub fn collect_memory_symbol_names(&self, file_id: i64) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT ms.memory_id, s.name, s.body_hash
             FROM memory_symbols ms
             JOIN symbols s ON s.id = ms.symbol_id
             WHERE s.file_id = ?1",
        )?;
        let rows = stmt.query_map(params![file_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Re-link memories to newly-inserted symbols by matching symbol name
    /// within `file_id`. Compares body hashes to detect semantic changes:
    /// - Name matches, body unchanged → clear stale, clear needs_review
    /// - Name matches, body changed → clear stale, set needs_review
    /// - Name gone → memory stays stale
    pub fn relink_memories_to_symbols(
        &self,
        file_id: i64,
        links: &[(i64, String, String)],
    ) -> Result<usize> {
        let mut relinked = 0;
        for (memory_id, sym_name, old_body_hash) in links {
            // Find the new symbol with the same name in the same file
            let new_sym: Option<(i64, String)> = self
                .conn
                .query_row(
                    "SELECT id, body_hash FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
                    params![file_id, sym_name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            if let Some((sid, new_body_hash)) = new_sym {
                self.conn.execute(
                    "INSERT OR IGNORE INTO memory_symbols (memory_id, symbol_id) VALUES (?1, ?2)",
                    params![memory_id, sid],
                )?;

                let body_changed = !old_body_hash.is_empty()
                    && !new_body_hash.is_empty()
                    && old_body_hash != &new_body_hash;

                if body_changed {
                    // Symbol name survived but implementation changed — flag for review
                    self.conn.execute(
                        "UPDATE memories SET stale = 0, needs_review = 1 WHERE id = ?1",
                        params![memory_id],
                    )?;
                } else {
                    // Symbol unchanged or hash not yet populated — clear both flags
                    self.conn.execute(
                        "UPDATE memories SET stale = 0, needs_review = 0 WHERE id = ?1",
                        params![memory_id],
                    )?;
                }
                relinked += 1;
            }
        }
        Ok(relinked)
    }

    /// Mark all memories linked to symbols in `file_id` as stale.
    pub fn mark_memories_stale_for_file(&self, file_id: i64) -> Result<usize> {
        let count = self.conn.execute(
            "UPDATE memories SET stale = 1
             WHERE id IN (
                 SELECT ms.memory_id FROM memory_symbols ms
                 JOIN symbols s ON s.id = ms.symbol_id
                 WHERE s.file_id = ?1
             )",
            params![file_id],
        )?;
        Ok(count)
    }

    /// Delete auto-observations older than `max_age_days` days.
    /// Manual memories are never cleaned up.
    pub fn cleanup_old_auto_observations(&self, max_age_days: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM memories
             WHERE source != 'manual'
               AND created_at < datetime('now', ?1)",
            params![format!("-{max_age_days} days")],
        )?;
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Session Recovery
    // -----------------------------------------------------------------------

    /// Reconstruct session state for post-compaction recovery.
    /// Returns manual memories (cross-session decisions), auto-observations for the
    /// target session, recently accessed files (via memory→symbol→file join), and
    /// symbol names that were accessed.
    pub fn get_session_recovery(&self, session_id: &str) -> Result<SessionRecoveryData> {
        // Manual memories are cross-session (session_id=''), always returned
        let manual_memories = {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.content, m.category, m.source, m.session_id,
                        m.created_at, m.stale, m.needs_review
                 FROM memories m
                 WHERE m.source = 'manual' AND m.stale = 0
                 ORDER BY m.created_at DESC
                 LIMIT 20",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    source: row.get(3)?,
                    session_id: row.get(4)?,
                    created_at: row.get(5)?,
                    stale: row.get::<_, i64>(6)? != 0,
                    needs_review: row.get::<_, i64>(7)? != 0,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        // Auto-observations for this specific session (chronological)
        let auto_observations = {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.content, m.category, m.source, m.session_id,
                        m.created_at, m.stale, m.needs_review
                 FROM memories m
                 WHERE m.session_id = ?1 AND m.source != 'manual' AND m.stale = 0
                 ORDER BY m.created_at ASC",
            )?;
            let rows = stmt.query_map(params![session_id], |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    source: row.get(3)?,
                    session_id: row.get(4)?,
                    created_at: row.get(5)?,
                    stale: row.get::<_, i64>(6)? != 0,
                    needs_review: row.get::<_, i64>(7)? != 0,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        // Recent files: memories → memory_symbols → symbols → files
        let recent_files = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT f.path
                 FROM memories m
                 JOIN memory_symbols ms ON ms.memory_id = m.id
                 JOIN symbols s ON s.id = ms.symbol_id
                 JOIN files f ON f.id = s.file_id
                 WHERE m.session_id = ?1 AND m.stale = 0
                 ORDER BY m.created_at DESC",
            )?;
            let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        // Symbol names accessed in this session (via memory_symbols links)
        let symbol_names_accessed = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT s.name
                 FROM memories m
                 JOIN memory_symbols ms ON ms.memory_id = m.id
                 JOIN symbols s ON s.id = ms.symbol_id
                 WHERE m.session_id = ?1 AND m.stale = 0
                 ORDER BY s.name",
            )?;
            let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(SessionRecoveryData {
            session_id: session_id.to_string(),
            manual_memories,
            auto_observations,
            recent_files,
            symbol_names_accessed,
        })
    }

    // -----------------------------------------------------------------------
    // FTS Search
    // -----------------------------------------------------------------------

    /// Rebuild the FTS5 index from the symbols table.
    pub fn rebuild_fts(&self) -> Result<()> {
        // FTS5 content-sync rebuild command re-reads all rows from the content table.
        self.conn.execute(
            "INSERT INTO symbols_fts(symbols_fts) VALUES ('rebuild')",
            [],
        )?;
        Ok(())
    }

    /// Full-text search over memories by content and category.
    pub fn search_memories(&self, query: &str, max_results: i64) -> Result<Vec<Memory>> {
        let fts_query: String = query
            .split_whitespace()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, m.category, m.source, m.session_id,
                    m.created_at, m.stale, m.needs_review
             FROM memories_fts fts
             JOIN memories m ON m.id = fts.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, max_results], |row| {
            Ok(Memory {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                session_id: row.get(4)?,
                created_at: row.get(5)?,
                stale: row.get::<_, i64>(6)? != 0,
                needs_review: row.get::<_, i64>(7)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Full-text search over symbols. Filters by kind and repo_id are optional.
    pub fn search_code(
        &self,
        query: &str,
        kind: &str,
        repo_id: Option<i64>,
        max_results: i64,
    ) -> Result<Vec<Symbol>> {
        // Sanitize for FTS5: wrap each token in double quotes to prevent
        // FTS5 operators (AND, OR, NOT, NEAR, *, +, -) from being interpreted.
        // Inner double-quotes are escaped by doubling them.
        let fts_query: String = query
            .split_whitespace()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut sql = String::from(
            "SELECT s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                    s.body_hash, s.start_line, s.end_line, s.parent_id
             FROM symbols_fts fts
             JOIN symbols s ON s.id = fts.rowid",
        );

        let need_repo_join = repo_id.is_some();
        if need_repo_join {
            sql.push_str(" JOIN files f ON f.id = s.file_id");
        }

        sql.push_str(" WHERE symbols_fts MATCH ?1");

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(fts_query));
        let mut idx = 2;

        if !kind.is_empty() {
            sql.push_str(&format!(" AND s.kind = ?{idx}"));
            param_values.push(Box::new(kind.to_string()));
            idx += 1;
        }
        if let Some(rid) = repo_id {
            sql.push_str(&format!(" AND f.repo_id = ?{idx}"));
            param_values.push(Box::new(rid));
            let _ = idx;
        }

        sql.push_str(" ORDER BY rank LIMIT ?");
        // We need the next param index
        let limit_idx = param_values.len() + 1;
        // Rewrite last push
        sql = sql.replace(
            " ORDER BY rank LIMIT ?",
            &format!(" ORDER BY rank LIMIT ?{limit_idx}"),
        );
        param_values.push(Box::new(max_results));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Symbol {
                id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                signature: row.get(4)?,
                body: row.get(5)?,
                body_hash: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                parent_id: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// FTS search with optional recency bias. When `recency_boost` > 0, files
    /// indexed within the last 48 hours get a ranking boost proportional to the
    /// value. Intended for debug-intent queries where recent changes correlate
    /// with the bug being investigated.
    pub fn search_code_with_recency(
        &self,
        query: &str,
        kind: &str,
        repo_id: Option<i64>,
        max_results: i64,
        recency_boost: f64,
    ) -> Result<Vec<Symbol>> {
        if recency_boost <= 0.0 {
            return self.search_code(query, kind, repo_id, max_results);
        }

        let fts_query: String = query
            .split_whitespace()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");

        // Recency-boosted ranking: multiply FTS5 rank by a decay factor based on
        // file indexed_at. Files touched within 48h get up to (1 + recency_boost)
        // multiplier; older files get 1.0 (no penalty).
        let mut sql = "SELECT s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                    s.body_hash, s.start_line, s.end_line, s.parent_id
             FROM symbols_fts fts
             JOIN symbols s ON s.id = fts.rowid
             JOIN files f ON f.id = s.file_id".to_string();

        if repo_id.is_some() {
            // already joined files
        }

        sql.push_str(" WHERE symbols_fts MATCH ?1");

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(fts_query));
        let mut idx = 2;

        if !kind.is_empty() {
            sql.push_str(&format!(" AND s.kind = ?{idx}"));
            param_values.push(Box::new(kind.to_string()));
            idx += 1;
        }
        if let Some(rid) = repo_id {
            sql.push_str(&format!(" AND f.repo_id = ?{idx}"));
            param_values.push(Box::new(rid));
            idx += 1;
        }

        // rank is negative in FTS5 (lower = better), so we multiply by a
        // factor < 1.0 for recent files to make them rank higher.
        // recency_factor: 1.0 for old files, (1 - boost*decay) for recent.
        sql.push_str(&format!(
            " ORDER BY rank * (1.0 - ?{idx} * MAX(0.0, \
             (julianday(f.indexed_at) - julianday('now', '-2 days')) / 2.0)) \
             LIMIT ?{}",
            idx + 1
        ));
        param_values.push(Box::new(recency_boost));
        param_values.push(Box::new(max_results));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Symbol {
                id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                signature: row.get(4)?,
                body: row.get(5)?,
                body_hash: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                parent_id: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Fuzzy name search using LIKE patterns derived from query terms.
    /// Used as fallback when FTS5 returns insufficient results — FTS5
    /// tokenizes on whitespace/punctuation and misses camelCase or partial
    /// symbol names that LIKE can catch.
    pub fn search_symbols_by_name_like(
        &self,
        terms: &[&str],
        repo_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<Symbol>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Build OR conditions for each term
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for (i, term) in terms.iter().enumerate() {
            conditions.push(format!("s.name LIKE ?{}", i + 1));
            param_values.push(Box::new(format!("%{term}%")));
        }

        let repo_join = if repo_id.is_some() {
            "JOIN files f ON f.id = s.file_id"
        } else {
            ""
        };
        let mut sql = format!(
            "SELECT s.id, s.file_id, s.name, s.kind, s.signature, s.body,
                    s.body_hash, s.start_line, s.end_line, s.parent_id
             FROM symbols s {repo_join} WHERE ({})",
            conditions.join(" OR ")
        );

        if let Some(rid) = repo_id {
            sql.push_str(&format!(" AND f.repo_id = ?{}", param_values.len() + 1));
            param_values.push(Box::new(rid));
        }
        sql.push_str(&format!(" LIMIT ?{}", param_values.len() + 1));
        param_values.push(Box::new(limit));

        let refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |row| {
            Ok(Symbol {
                id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                signature: row.get(4)?,
                body: row.get(5)?,
                body_hash: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                parent_id: row.get(9)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Stats / Overview
    // -----------------------------------------------------------------------

    pub fn get_health(&self) -> Result<HealthReport> {
        let db_size: i64 = self
            .conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count, pragma_page_size",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let symbol_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
        let file_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let edge_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
        let memory_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        let repo_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM repositories", [], |r| r.get(0))?;
        let fts_ok = self
            .conn
            .execute(
                "INSERT INTO symbols_fts(symbols_fts) VALUES ('integrity-check')",
                [],
            )
            .is_ok();
        Ok(HealthReport {
            db_size_bytes: db_size,
            symbol_count,
            file_count,
            edge_count,
            memory_count,
            repo_count,
            fts_ok,
        })
    }

    pub fn get_repo_overview(&self, repo_name: &str) -> Result<Vec<RepoOverview>> {
        let mut sql = String::from(
            "SELECT r.id, r.name, r.root_path FROM repositories r",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if !repo_name.is_empty() {
            sql.push_str(" WHERE r.name = ?1");
            param_values.push(Box::new(repo_name.to_string()));
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let repos: Vec<(i64, String, String)> = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::new();
        for (repo_id, name, root_path) in repos {
            let file_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM files WHERE repo_id = ?1",
                params![repo_id],
                |r| r.get(0),
            )?;

            let symbol_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM symbols s
                 JOIN files f ON f.id = s.file_id
                 WHERE f.repo_id = ?1",
                params![repo_id],
                |r| r.get(0),
            )?;

            let memory_count: i64 = self.conn.query_row(
                "SELECT COUNT(DISTINCT m.id) FROM memories m
                 JOIN memory_symbols ms ON ms.memory_id = m.id
                 JOIN symbols s ON s.id = ms.symbol_id
                 JOIN files f ON f.id = s.file_id
                 WHERE f.repo_id = ?1",
                params![repo_id],
                |r| r.get(0),
            )?;

            let mut lang_stmt = self.conn.prepare(
                "SELECT language, COUNT(*) as cnt FROM files
                 WHERE repo_id = ?1 GROUP BY language ORDER BY cnt DESC",
            )?;
            let languages: Vec<LanguageCount> = lang_stmt
                .query_map(params![repo_id], |row| {
                    Ok(LanguageCount {
                        language: row.get(0)?,
                        count: row.get(1)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            out.push(RepoOverview {
                name,
                root_path,
                file_count,
                symbol_count,
                memory_count,
                languages,
            });
        }

        Ok(out)
    }

    /// Return symbols in a file as summaries (no body), optionally scoped to a repo.
    /// Matches file path with a LIKE suffix pattern so callers can pass relative paths.
    pub fn get_file_symbols_summary(
        &self,
        file_path: &str,
        repo_name: Option<&str>,
    ) -> Result<Vec<SymbolSummary>> {
        let mut sql = String::from(
            "SELECT s.name, s.kind, s.signature, s.start_line, s.end_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             JOIN repositories r ON r.id = f.repo_id
             WHERE f.path LIKE ?1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let pattern = format!("%{file_path}");
        param_values.push(Box::new(pattern));

        if let Some(rn) = repo_name {
            sql.push_str(" AND r.name = ?2");
            param_values.push(Box::new(rn.to_string()));
        }

        sql.push_str(" ORDER BY s.start_line");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(SymbolSummary {
                name: row.get(0)?,
                kind: row.get(1)?,
                signature: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Skeleton Mode
    // -----------------------------------------------------------------------

    /// Return symbols in a file as summaries (signatures only, no body).
    /// The `detail` param accepts "minimal", "standard", or "verbose" -- for v1
    /// all levels return the same thing (signatures + line ranges).
    pub fn get_skeleton(&self, file_id: i64, _detail: &str) -> Result<Vec<SymbolSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, kind, signature, start_line, end_line
             FROM symbols WHERE file_id = ?1 ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![file_id], |row| {
            Ok(SymbolSummary {
                name: row.get(0)?,
                kind: row.get(1)?,
                signature: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Find a file by path suffix match (LIKE %path), optionally scoped to a repo
    /// by name, then return its skeleton (signatures only).
    pub fn get_skeleton_by_path(
        &self,
        file_path: &str,
        repo_name: Option<&str>,
        detail: &str,
    ) -> Result<Vec<SymbolSummary>> {
        let mut sql = String::from(
            "SELECT f.id FROM files f
             JOIN repositories r ON r.id = f.repo_id
             WHERE f.path LIKE ?1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let pattern = format!("%{file_path}");
        param_values.push(Box::new(pattern));

        if let Some(rn) = repo_name {
            sql.push_str(" AND r.name = ?2");
            param_values.push(Box::new(rn.to_string()));
        }

        sql.push_str(" LIMIT 1");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let file_id: Option<i64> = self
            .conn
            .query_row(&sql, params_refs.as_slice(), |row| row.get(0))
            .optional()?;

        match file_id {
            Some(id) => self.get_skeleton(id, detail),
            None => Ok(Vec::new()),
        }
    }

    /// Return all user table names (for testing/diagnostics).
    pub fn table_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'virtual table')
             AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}
