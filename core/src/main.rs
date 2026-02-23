use std::path::PathBuf;
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
    #[arg(required = true)]
    paths: Vec<std::path::PathBuf>,

    /// Run HTTP MCP server instead of stdio
    #[arg(long)]
    http: bool,

    /// HTTP port (only with --http)
    #[arg(long, default_value = "3100")]
    port: u16,
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

    // Index each workspace root
    let registry = GrammarRegistry::new();
    let indexer = Indexer::new(&db, &registry);

    for path in &cli.paths {
        tracing::info!(path = %path.display(), "indexing workspace");
        match indexer.index_directory(path) {
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

    // Wrap DB in Arc<Mutex<>> for the MCP server
    let db = Arc::new(Mutex::new(db));
    let workspace_roots: Vec<_> = cli.paths.clone();

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

        let service: StreamableHttpService<FocalServer, LocalSessionManager> =
            StreamableHttpService::new(
                {
                    let db = Arc::clone(&db);
                    let roots = workspace_roots.clone();
                    move || Ok(FocalServer::new(Arc::clone(&db), roots.clone()))
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
    let server = FocalServer::new(db, workspace_roots);
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;

    Ok(())
}
