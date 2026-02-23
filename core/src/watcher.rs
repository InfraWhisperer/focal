use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Watches workspace directories for file changes with debouncing.
///
/// Uses the platform-native backend (FSEvents on macOS, inotify on Linux, etc.)
/// and coalesces rapid-fire events into batched, deduplicated path lists.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<Vec<PathBuf>>,
}

impl FileWatcher {
    /// Watch the given root directories for file changes.
    ///
    /// `debounce_ms` controls how long to wait after the first event before
    /// flushing the batch. Events arriving within that window are coalesced.
    pub fn new(roots: &[PathBuf], debounce_ms: u64) -> Result<Self> {
        let (raw_tx, raw_rx) = mpsc::channel::<Event>();
        let (batch_tx, batch_rx) = mpsc::channel::<Vec<PathBuf>>();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let _ = raw_tx.send(event);
                }
            },
            Config::default(),
        )
        .context("failed to create filesystem watcher")?;

        for root in roots {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .with_context(|| format!("failed to watch {}", root.display()))?;
        }

        let debounce = Duration::from_millis(debounce_ms);

        // Background thread: coalesce raw events into debounced batches.
        std::thread::Builder::new()
            .name("file-watcher-debounce".into())
            .spawn(move || {
                loop {
                    // Block until the first event arrives.
                    let first = match raw_rx.recv() {
                        Ok(ev) => ev,
                        Err(_) => return, // channel closed, watcher dropped
                    };

                    let mut paths = HashSet::new();
                    collect_file_paths(&first, &mut paths);

                    // Drain any additional events that arrive within the debounce window.
                    let deadline = std::time::Instant::now() + debounce;
                    loop {
                        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        match raw_rx.recv_timeout(remaining) {
                            Ok(ev) => collect_file_paths(&ev, &mut paths),
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                // Send whatever we have, then exit.
                                if !paths.is_empty() {
                                    let _ = batch_tx.send(paths.into_iter().collect());
                                }
                                return;
                            }
                        }
                    }

                    if !paths.is_empty()
                        && batch_tx.send(paths.into_iter().collect()).is_err()
                    {
                        return; // receiver dropped
                    }
                }
            })
            .context("failed to spawn debounce thread")?;

        Ok(Self {
            _watcher: watcher,
            rx: batch_rx,
        })
    }

    /// Block until changed files arrive (up to `timeout`).
    ///
    /// Returns a deduplicated list of changed file paths, or an empty vec on timeout.
    pub fn wait_for_changes(&self, timeout: Duration) -> Vec<PathBuf> {
        self.rx.recv_timeout(timeout).unwrap_or_default()
    }
}

/// Extract file paths from a notify event, filtering out directories.
fn collect_file_paths(event: &Event, out: &mut HashSet<PathBuf>) {
    for path in &event.paths {
        // Only include actual files, not directories.
        // Use symlink_metadata to avoid following symlinks -- if the path
        // doesn't exist anymore (deleted), include it anyway since the
        // caller needs to know about deletions.
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => continue,
            _ => {
                out.insert(path.clone());
            }
        }
    }
}
