use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove test directory: {error}");
        }
    }
}

struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(None) => {
                if let Err(error) = self.0.kill() {
                    eprintln!("could not stop watcher: {error}");
                }
            }
            Ok(Some(_)) => {}
            Err(error) => eprintln!("could not check watcher: {error}"),
        }
        if let Err(error) = self.0.wait() {
            eprintln!("could not reap watcher: {error}");
        }
    }
}

#[test]
fn watch_updates_snapshot_for_changes_before_and_after_start() -> Result<(), Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!(
        "rfsync-cli-watch-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
    ));
    fs::create_dir(&path)?;
    let dir = TestDir(path);
    let root = dir.0.join("root");
    fs::create_dir(&root)?;
    fs::write(root.join("file"), b"old")?;
    let binary = env!("CARGO_BIN_EXE_rfsync");
    let added = Command::new(binary)
        .current_dir(&dir.0)
        .args(["add", "root"])
        .output()?;
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    fs::write(root.join("file"), b"changed")?;
    fs::write(root.join("offline"), b"offline")?;

    let conn = rfsync_storage::open(&dir.0.join("example.db"))?;
    let root_path = root.canonicalize()?;
    let root_info =
        rfsync_storage::get_root_by_path(&conn, root_path.to_str().ok_or("invalid path")?)?
            .ok_or("root not registered")?;
    let mut watcher = Watcher(
        Command::new(binary)
            .current_dir(&dir.0)
            .args(["watch", "root"])
            .spawn()?,
    );

    let mut wait_for = |expected: &[(&str, i64)]| -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut files: Vec<_> = rfsync_storage::get_files_by_root_id(&conn, root_info.id)?
                .into_iter()
                .map(|file| (file.name, file.size))
                .collect();
            files.sort();
            if files
                .iter()
                .map(|(name, size)| (name.as_str(), *size))
                .eq(expected.iter().copied())
            {
                return Ok(());
            }
            if let Some(status) = watcher.0.try_wait()? {
                return Err(format!("watch exited: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err(format!("watch did not synchronize: {files:?}").into());
            }
            thread::sleep(Duration::from_millis(25));
        }
    };
    wait_for(&[("file", 7), ("offline", 7)])?;
    fs::write(root.join("live"), b"live")?;
    wait_for(&[("file", 7), ("live", 4), ("offline", 7)])?;
    Ok(())
}
