use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rfsync_engine::{create_initial_snapshot, register_root, watch_root};

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove test directory: {error}");
        }
    }
}

struct WatcherProcess(Child);

impl Drop for WatcherProcess {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            eprintln!("could not stop watcher: {error}");
        }
        if let Err(error) = self.0.wait() {
            eprintln!("could not reap watcher: {error}");
        }
    }
}

#[test]
fn watcher_child() -> Result<(), Box<dyn Error>> {
    let Some(db) = std::env::var_os("RFSYNC_TEST_WATCH_DB") else {
        return Ok(());
    };
    let root_id = std::env::var("RFSYNC_TEST_WATCH_ROOT_ID")?.parse()?;
    watch_root(root_id, Path::new(&db))?;
    Ok(())
}

#[test]
fn watcher_updates_snapshot_for_real_file_events() -> Result<(), Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!(
        "rfsync-watch-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
    ));
    fs::create_dir(&path)?;
    let dir = TestDir(path);
    let root = dir.0.join("root");
    let db = dir.0.join("snapshot.db");
    fs::create_dir(&root)?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;

    let mut watcher = WatcherProcess(
        Command::new(std::env::current_exe()?)
            .args(["--exact", "watcher_child", "--nocapture"])
            .env("RFSYNC_TEST_WATCH_DB", &db)
            .env("RFSYNC_TEST_WATCH_ROOT_ID", root_id.to_string())
            .spawn()?,
    );
    let conn = rfsync_storage::open(&db)?;
    let root_folder =
        rfsync_storage::get_folder_by_path(&conn, root_id, "")?.ok_or("missing root folder")?;
    let file_size = |name: &str| -> Result<Option<i64>, Box<dyn Error>> {
        Ok(
            rfsync_storage::get_file_by_folder_id_and_name(&conn, root_id, root_folder.id, name)?
                .map(|file| file.size),
        )
    };

    // An observed event confirms the watcher is ready before the actual scenario.
    let deadline = Instant::now() + Duration::from_secs(10);
    while file_size("ready")?.is_none() {
        if let Some(status) = watcher.0.try_wait()? {
            return Err(format!("watcher exited before readiness: {status}").into());
        }
        if Instant::now() >= deadline {
            return Err("watcher did not become ready".into());
        }
        fs::write(root.join("ready"), b"ready")?;
        thread::sleep(Duration::from_millis(50));
    }

    let wait_for = |name: &str, expected: Option<i64>| -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if file_size(name)? == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting for {name}: expected {expected:?}").into());
            }
            thread::sleep(Duration::from_millis(25));
        }
    };

    fs::write(root.join("file"), b"new")?;
    wait_for("file", Some(3))?;
    fs::write(root.join("file"), b"modified")?;
    wait_for("file", Some(8))?;
    fs::rename(root.join("file"), root.join("renamed"))?;
    wait_for("file", None)?;
    wait_for("renamed", Some(8))?;
    fs::remove_file(root.join("renamed"))?;
    wait_for("renamed", None)?;
    Ok(())
}
