use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::Database;

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub repo: String,
    pub exported_at: String,
    pub focal_version: String,
    pub symbols: Vec<ManifestSymbol>,
    pub edges: Vec<ManifestEdge>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file: String,
    pub line_start: i64,
    pub line_end: i64,
    pub signature: String,
    pub language: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

pub fn export_manifest(db: &Database, repo_id: i64, repo_name: &str) -> Result<Manifest> {
    let symbols_raw = db.export_symbols_for_repo(repo_id)?;
    let edges_raw = db.export_edges_for_repo(repo_id)?;

    let symbols: Vec<ManifestSymbol> = symbols_raw
        .into_iter()
        .map(|(sym, file_path, language)| ManifestSymbol {
            name: sym.name,
            qualified_name: sym.qualified_name,
            kind: sym.kind,
            file: file_path,
            line_start: sym.start_line,
            line_end: sym.end_line,
            signature: sym.signature,
            language,
        })
        .collect();

    let edges: Vec<ManifestEdge> = edges_raw
        .into_iter()
        .filter(|(src, tgt, _)| !src.is_empty() && !tgt.is_empty())
        .map(|(source, target, kind)| ManifestEdge {
            source,
            target,
            kind,
        })
        .collect();

    let exported_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();

    Ok(Manifest {
        version: MANIFEST_VERSION,
        repo: repo_name.to_string(),
        exported_at,
        focal_version: env!("CARGO_PKG_VERSION").to_string(),
        symbols,
        edges,
    })
}

/// Parse a manifest from a file path.
pub fn load_manifest(path: &Path) -> Result<Manifest> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse manifest at {}", path.display()))?;
    if manifest.version != MANIFEST_VERSION {
        anyhow::bail!(
            "unsupported manifest version {} (expected {})",
            manifest.version,
            MANIFEST_VERSION
        );
    }
    Ok(manifest)
}

/// Fetch a manifest from a URL (raw git file URL).
pub fn fetch_manifest(url: &str) -> Result<Manifest> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build(),
    );
    let body = agent
        .get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("HTTP request failed for {url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?;

    let manifest: Manifest = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse manifest from {url}"))?;

    if manifest.version != MANIFEST_VERSION {
        anyhow::bail!(
            "unsupported manifest version {} from {url} (expected {MANIFEST_VERSION})",
            manifest.version,
        );
    }

    Ok(manifest)
}

/// Import a manifest into the database. Returns (symbol_count, edge_count).
pub fn import_manifest(db: &Database, manifest: &Manifest) -> Result<(usize, usize)> {
    let repo_name = &manifest.repo;

    db.with_transaction(|| {
        // Clean reimport: delete old symbols/edges for this manifest repo
        let deleted = db.delete_manifest_symbols(repo_name)?;
        if deleted > 0 {
            tracing::info!(deleted, repo = %repo_name, "removed previous manifest symbols");
        }

        // Create a "virtual" repo for the manifest source
        let repo_id = db.upsert_repository(repo_name, &format!("manifest://{repo_name}"))?;

        // Group symbols by file to create file records
        let mut files_seen: HashMap<String, i64> = HashMap::new();
        let mut qname_to_id: HashMap<String, i64> = HashMap::new();
        let mut symbol_count = 0;

        for sym in &manifest.symbols {
            let file_id = match files_seen.get(&sym.file) {
                Some(&id) => id,
                None => {
                    let id = db.upsert_file(repo_id, &sym.file, &sym.language, "manifest")?;
                    files_seen.insert(sym.file.clone(), id);
                    id
                }
            };

            // Local takes precedence — skip manifest symbol if collision
            if !sym.qualified_name.is_empty()
                && db.find_symbol_by_qualified_name_local(&sym.qualified_name)?
            {
                continue;
            }

            let sym_id = db.insert_manifest_symbol(
                file_id,
                &sym.name,
                &sym.qualified_name,
                &sym.kind,
                &sym.signature,
                sym.line_start,
                sym.line_end,
                repo_name,
            )?;

            qname_to_id.insert(sym.qualified_name.clone(), sym_id);
            symbol_count += 1;
        }

        // Resolve and insert edges
        let mut edge_count = 0;
        for edge in &manifest.edges {
            let source_id = qname_to_id.get(&edge.source);
            let target_id = qname_to_id.get(&edge.target);
            if let (Some(&src), Some(&tgt)) = (source_id, target_id) {
                db.insert_edge(src, tgt, &edge.kind)?;
                edge_count += 1;
            }
        }

        // Record manifest metadata
        db.upsert_manifest(
            repo_name,
            manifest.version,
            &manifest.focal_version,
            &manifest.exported_at,
            symbol_count,
            edge_count,
        )?;

        Ok((symbol_count, edge_count))
    })
}
