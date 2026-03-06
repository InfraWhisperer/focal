use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use focal_core::db::Database;
use focal_core::grammar::GrammarRegistry;
use focal_core::indexer::Indexer;
use focal_core::mcp::FocalServer;
use focal_core::watcher::FileWatcher;

#[derive(Parser)]
#[command(name = "focal", about = "Structural code index for Claude Code")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Workspace root paths to index (backwards-compatible shorthand for `focal serve`)
    #[arg(global = false)]
    paths: Vec<PathBuf>,

    /// Run HTTP MCP server instead of stdio
    #[arg(long)]
    http: bool,

    /// HTTP port (only with --http)
    #[arg(long, default_value = "3100")]
    port: u16,
}

#[derive(Subcommand)]
enum Commands {
    /// Index workspace(s) and serve MCP (default behavior)
    Serve {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        http: bool,
        #[arg(long, default_value = "3100")]
        port: u16,
    },
    /// Run interactive setup wizard
    Init,
    /// Export symbol manifest for the current repo
    Export {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Import symbol manifest(s) from another repo
    Import {
        source: Option<PathBuf>,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        git: Option<String>,
    },
}

fn run_init_wizard() -> anyhow::Result<()> {
    use std::io::{self, BufRead, Write};

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Detect workspace root
    let cwd = std::env::current_dir()?;
    eprint!("Workspace path [{}]: ", cwd.display());
    stdout.flush()?;
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;
    let workspace = input.trim();
    let workspace_path = if workspace.is_empty() {
        cwd.clone()
    } else {
        PathBuf::from(workspace).canonicalize()?
    };

    // Resolve binary path
    let binary_path = std::env::current_exe()?;

    // Write .mcp.json in the workspace root
    let mcp_json_path = workspace_path.join(".mcp.json");
    let config = serde_json::json!({
        "mcpServers": {
            "focal": {
                "command": binary_path.to_string_lossy(),
                "args": [workspace_path.to_string_lossy()]
            }
        }
    });
    let config_str = serde_json::to_string_pretty(&config)?;

    eprintln!("\nWill write to {}:", mcp_json_path.display());
    eprintln!("{config_str}");
    eprint!("\nProceed? [Y/n]: ");
    stdout.flush()?;
    let mut confirm = String::new();
    stdin.lock().read_line(&mut confirm)?;
    if confirm.trim().eq_ignore_ascii_case("n") {
        eprintln!("Aborted.");
        return Ok(());
    }

    std::fs::write(&mcp_json_path, config_str)?;
    eprintln!("\nWrote {}", mcp_json_path.display());
    eprintln!("Claude Code will pick up Focal on its next session in this workspace.");

    Ok(())
}

fn run_export(path: PathBuf, output: Option<PathBuf>) -> anyhow::Result<()> {
    let workspace = path.canonicalize()?;

    let db_dir = dirs::home_dir()
        .expect("failed to determine home directory")
        .join(".focal");
    let db_path = db_dir.join("index.db");

    if !db_path.exists() {
        anyhow::bail!(
            "no Focal database found at {}. Run 'focal serve' first.",
            db_path.display()
        );
    }

    let db = focal_core::db::Database::open(&db_path.to_string_lossy())?;

    let repo = db
        .get_repository_by_path(&workspace.to_string_lossy())?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no index found for {}. Run 'focal serve {}' first.",
                workspace.display(),
                workspace.display()
            )
        })?;

    let manifest = focal_core::manifest::export_manifest(&db, repo.id, &repo.name)?;

    let out_path = match output {
        Some(p) => p,
        None => {
            let focal_dir = workspace.join(".focal");
            std::fs::create_dir_all(&focal_dir)?;
            focal_dir.join("manifest.json")
        }
    };

    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&out_path, &json)?;

    eprintln!(
        "Exported {} symbols and {} edges to {}",
        manifest.symbols.len(),
        manifest.edges.len(),
        out_path.display()
    );

    Ok(())
}

fn run_import(
    source: Option<PathBuf>,
    dir: Option<PathBuf>,
    git: Option<String>,
) -> anyhow::Result<()> {
    let db_dir = dirs::home_dir()
        .expect("failed to determine home directory")
        .join(".focal");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("index.db");
    let db = focal_core::db::Database::open(&db_path.to_string_lossy())?;

    let mut manifests_to_import: Vec<PathBuf> = Vec::new();

    if let Some(src) = source {
        manifests_to_import.push(src);
    }

    if let Some(d) = dir {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                manifests_to_import.push(path);
            }
        }
    }

    let git_imported = if let Some(url) = git {
        let manifest = focal_core::manifest::fetch_manifest(&url)?;
        let (syms, edges) = focal_core::manifest::import_manifest(&db, &manifest)?;
        eprintln!(
            "Imported {} symbols and {} edges from {}",
            syms, edges, manifest.repo
        );
        true
    } else {
        false
    };

    if manifests_to_import.is_empty() && !git_imported {
        anyhow::bail!(
            "no manifest source specified. Use: focal import <path>, --dir <dir>, or --git <url>"
        );
    }

    for path in &manifests_to_import {
        let manifest = focal_core::manifest::load_manifest(path)?;
        let (syms, edges) = focal_core::manifest::import_manifest(&db, &manifest)?;
        eprintln!(
            "Imported {} symbols and {} edges from {}",
            syms, edges, manifest.repo
        );
    }

    Ok(())
}

async fn run_serve(paths: Vec<PathBuf>, http: bool, port: u16) -> anyhow::Result<()> {
    tracing::info!(?paths, "starting focal");

    // Resolve DB path: ~/.focal/index.db
    let db_dir = dirs::home_dir()
        .expect("failed to determine home directory")
        .join(".focal");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("index.db");
    let db_path_str = db_path.to_string_lossy().to_string();

    tracing::info!(db = %db_path_str, "opening database");
    let db = Database::open(&db_path_str)?;

    // Clean up auto-observations older than 90 days
    let cleaned = db.cleanup_old_auto_observations(90)?;
    if cleaned > 0 {
        tracing::info!(cleaned, "purged old auto-observations");
    }

    // Wrap DB in Arc<Mutex<>> before spawning background work
    let db = Arc::new(Mutex::new(db));
    let workspace_roots: Vec<_> = paths.clone();

    // Index each workspace root in the background so MCP starts serving immediately
    let indexing_complete = Arc::new(AtomicBool::new(false));
    {
        let db_clone = Arc::clone(&db);
        let paths = paths.clone();
        let indexing_complete_clone = Arc::clone(&indexing_complete);
        tokio::task::spawn_blocking(move || {
            let registry = GrammarRegistry::new();
            for path in &paths {
                tracing::info!(path = %path.display(), "indexing workspace");
                let result = {
                    let db = match db_clone.lock() {
                        Ok(db) => db,
                        Err(e) => {
                            tracing::error!(error = %e, "failed to lock DB for indexing");
                            continue;
                        }
                    };
                    let indexer = Indexer::new(&db, &registry);
                    indexer.index_directory(path)
                };
                match result {
                    Ok(stats) => {
                        tracing::info!(
                            files_indexed = stats.files_indexed,
                            files_skipped = stats.files_skipped,
                            symbols = stats.symbols_extracted,
                            edges = stats.edges_created,
                            errors = stats.errors.len(),
                            "indexing complete"
                        );
                        for err in &stats.errors {
                            tracing::warn!(error = %err, "indexer error");
                        }
                    }
                    Err(e) => {
                        tracing::error!(path = %path.display(), error = %e, "failed to index workspace");
                    }
                }
            }
            indexing_complete_clone.store(true, Ordering::Relaxed);
            tracing::info!("background indexing finished");
        });
    }

    // Auto-import manifests from config
    {
        let db_clone = Arc::clone(&db);
        tokio::task::spawn_blocking(move || {
            let config = focal_core::config::FocalConfig::load();

            let has_work = !config.manifests.auto_import.is_empty()
                || !config.manifests.auto_import_git.is_empty();
            if !has_work {
                return;
            }

            let db = match db_clone.lock() {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(error = %e, "failed to lock DB for auto-import");
                    return;
                }
            };

            // Filesystem imports
            for path_str in &config.manifests.auto_import {
                let path = std::path::Path::new(path_str);
                if path.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(path) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().is_some_and(|e| e == "json") {
                                match focal_core::manifest::load_manifest(&p) {
                                    Ok(m) => match focal_core::manifest::import_manifest(&db, &m) {
                                        Ok((s, e)) => tracing::info!(symbols = s, edges = e, repo = %m.repo, "auto-imported manifest"),
                                        Err(e) => tracing::warn!(error = %e, path = %p.display(), "manifest import failed"),
                                    },
                                    Err(e) => tracing::warn!(error = %e, path = %p.display(), "manifest load failed"),
                                }
                            }
                        }
                    }
                } else if path.is_file() {
                    match focal_core::manifest::load_manifest(path) {
                        Ok(m) => match focal_core::manifest::import_manifest(&db, &m) {
                            Ok((s, e)) => tracing::info!(symbols = s, edges = e, repo = %m.repo, "auto-imported manifest"),
                            Err(e) => tracing::warn!(error = %e, path = %path_str, "manifest import failed"),
                        },
                        Err(e) => tracing::warn!(error = %e, path = %path_str, "manifest load failed"),
                    }
                }
            }

            // Git imports — skip silently on network failure
            for url in &config.manifests.auto_import_git {
                if let Ok(m) = focal_core::manifest::fetch_manifest(url) {
                    match focal_core::manifest::import_manifest(&db, &m) {
                        Ok((s, e)) => tracing::info!(symbols = s, edges = e, repo = %m.repo, "auto-imported git manifest"),
                        Err(e) => tracing::warn!(error = %e, url = %url, "git manifest import failed"),
                    }
                }
            }
        });
    }

    // Spawn file watcher for incremental re-indexing
    {
        let db_clone = Arc::clone(&db);
        let roots: Vec<PathBuf> = paths.clone();
        let registry = GrammarRegistry::new();
        tokio::spawn(async move {
            let watcher = match FileWatcher::new(&roots, 500) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(error = %e, "failed to start file watcher");
                    return;
                }
            };
            tracing::info!("file watcher started");
            loop {
                let changed = watcher.wait_for_changes(Duration::from_secs(60));
                if changed.is_empty() {
                    continue;
                }
                let mut reindexed = 0;
                let mut removed = 0;
                for path in &changed {
                    let root = roots.iter().find(|r| path.starts_with(r));
                    if let Some(root) = root {
                        // Lock per-file to avoid blocking MCP handlers for the
                        // entire batch duration.
                        if !path.exists() {
                            // File was deleted — clean up stale symbols/edges
                            let result = {
                                let db = match db_clone.lock() {
                                    Ok(db) => db,
                                    Err(e) => {
                                        tracing::error!(error = %e, "failed to lock DB");
                                        continue;
                                    }
                                };
                                let indexer = Indexer::new(&db, &registry);
                                indexer.remove_deleted_file(path, root)
                            };
                            match result {
                                Ok(true) => removed += 1,
                                Ok(false) => {}
                                Err(e) => tracing::warn!(path = %path.display(), error = %e, "remove error"),
                            }
                            continue;
                        }
                        let result = {
                            let db = match db_clone.lock() {
                                Ok(db) => db,
                                Err(e) => {
                                    tracing::error!(error = %e, "failed to lock DB for re-index");
                                    continue;
                                }
                            };
                            let indexer = Indexer::new(&db, &registry);
                            indexer.index_file(path, root)
                        };
                        match result {
                            Ok(true) => reindexed += 1,
                            Ok(false) => {}
                            Err(e) => tracing::warn!(path = %path.display(), error = %e, "re-index error"),
                        }
                    }
                }
                if reindexed > 0 || removed > 0 {
                    tracing::info!(reindexed, removed, "file watcher processed changes");
                }
            }
        });
    }

    if http {
        let ct = CancellationToken::new();

        let indexing_complete_http = Arc::clone(&indexing_complete);
        let service: StreamableHttpService<FocalServer, LocalSessionManager> =
            StreamableHttpService::new(
                {
                    let db = Arc::clone(&db);
                    let roots = workspace_roots.clone();
                    move || Ok(FocalServer::new(Arc::clone(&db), roots.clone(), Arc::clone(&indexing_complete_http)))
                },
                Default::default(),
                StreamableHttpServerConfig {
                    stateful_mode: true,
                    cancellation_token: ct.child_token(),
                    ..Default::default()
                },
            );

        let router = axum::Router::new().nest_service("/mcp", service);
        let bind_addr = format!("127.0.0.1:{port}");
        let tcp_listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        tracing::info!(addr = %bind_addr, "serving MCP over HTTP");

        axum::serve(tcp_listener, router)
            .with_graceful_shutdown(async move {
                tokio::signal::ctrl_c()
                    .await
                    .expect("failed to install CTRL+C handler");
                tracing::info!("shutting down HTTP MCP server");
                ct.cancel();
            })
            .await?;

        return Ok(());
    }

    // Serve MCP over stdio
    tracing::info!("serving MCP over stdio");
    let server = FocalServer::new(db, workspace_roots, Arc::clone(&indexing_complete));
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("focal=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve { paths, http, port }) => {
            run_serve(paths, http, port).await
        }
        Some(Commands::Init) => run_init_wizard(),
        Some(Commands::Export { path, output }) => run_export(path, output),
        Some(Commands::Import { source, dir, git }) => run_import(source, dir, git),
        None => {
            // Backwards compat: bare `focal /path [--http] [--port N]` maps to serve
            if cli.paths.is_empty() {
                // No subcommand and no paths — print help
                use clap::CommandFactory;
                Cli::command().print_help()?;
                std::process::exit(0);
            }
            run_serve(cli.paths, cli.http, cli.port).await
        }
    }
}
