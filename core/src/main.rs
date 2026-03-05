use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
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
struct Cli {
    /// Workspace root paths to index
    #[arg(required_unless_present = "init")]
    paths: Vec<std::path::PathBuf>,

    /// Run interactive setup wizard
    #[arg(long)]
    init: bool,

    /// Run HTTP MCP server instead of stdio
    #[arg(long)]
    http: bool,

    /// HTTP port (only with --http)
    #[arg(long, default_value = "3100")]
    port: u16,
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

    if cli.init {
        return run_init_wizard();
    }

    tracing::info!(paths = ?cli.paths, "starting focal");

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
    let workspace_roots: Vec<_> = cli.paths.clone();

    // Index each workspace root in the background so MCP starts serving immediately
    let indexing_complete = Arc::new(AtomicBool::new(false));
    {
        let db_clone = Arc::clone(&db);
        let paths = cli.paths.clone();
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

    // Spawn file watcher for incremental re-indexing
    {
        let db_clone = Arc::clone(&db);
        let roots: Vec<PathBuf> = cli.paths.clone();
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

    if cli.http {
        let port = cli.port;
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
