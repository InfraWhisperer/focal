use std::fs;
use std::time::Duration;

use tempfile::tempdir;
use focal_core::watcher::FileWatcher;

#[test]
fn test_watcher_detects_file_changes() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "initial content").unwrap();

    let watcher = FileWatcher::new(&[dir.path().to_path_buf()], 100).unwrap();

    // Give the watcher time to initialize and register with the OS backend.
    std::thread::sleep(Duration::from_millis(200));

    // Modify the file.
    fs::write(&file_path, "modified content").unwrap();

    let changed = watcher.wait_for_changes(Duration::from_secs(2));
    assert!(
        !changed.is_empty(),
        "expected at least one changed path, got none"
    );

    // On macOS FSEvents may report the canonical (resolved) path.
    let canonical = file_path.canonicalize().unwrap();
    assert!(
        changed.iter().any(|p| *p == file_path || *p == canonical),
        "expected changed paths to contain {}, got: {:?}",
        file_path.display(),
        changed
    );
}
