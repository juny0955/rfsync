use std::{
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rfsync_engine::{DeviceId, register_root, watch_root};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rfsync-watch-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove test directory: {error}");
        }
    }
}

struct WatcherProcess(Child);

impl WatcherProcess {
    fn start(db: &Path, root_id: i64) -> Result<Self, Box<dyn Error>> {
        Ok(Self(
            Command::new(std::env::current_exe()?)
                .args(["--exact", "감시기_자식_프로세스", "--nocapture"])
                .env("RFSYNC_TEST_WATCH_DB", db)
                .env("RFSYNC_TEST_WATCH_ROOT_ID", root_id.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()?,
        ))
    }
}

impl Drop for WatcherProcess {
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
fn 감시기_자식_프로세스() -> Result<(), Box<dyn Error>> {
    let Some(db) = std::env::var_os("RFSYNC_TEST_WATCH_DB") else {
        return Ok(());
    };
    let root_id = std::env::var("RFSYNC_TEST_WATCH_ROOT_ID")?.parse()?;
    watch_root(root_id, Path::new(&db), DeviceId::from_cert(b"test"))?;
    Ok(())
}

#[test]
fn 감시기는_실제_파일_이벤트로_스냅샷을_갱신한다() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("snapshot.db");
    fs::create_dir(&root)?;
    let root_id = register_root(&root, &db, rfsync_engine::DeviceId::from_cert(b"test"))?;

    let mut watcher = WatcherProcess::start(&db, root_id)?;
    let conn = rfsync_storage::open(&db)?;
    let root_folder =
        rfsync_storage::get_folder_by_path(&conn, root_id, "")?.ok_or("missing root folder")?;
    let file_size = |name: &str| -> Result<Option<i64>, Box<dyn Error>> {
        Ok(
            rfsync_storage::get_file_by_folder_id_and_name(&conn, root_id, root_folder.id, name)?
                .filter(|file| !file.deleted)
                .map(|file| file.size),
        )
    };

    // 감시자가 이벤트를 관측했다면 준비된 것이므로, 본 시나리오는 그 뒤에 시작한다.
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

#[test]
fn 감시기는_시작_전에_발생한_변경을_맞춰_반영한다() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("modified"), b"old")?;
    fs::write(root.join("deleted"), b"old")?;
    let root_id = register_root(&root, &db, rfsync_engine::DeviceId::from_cert(b"test"))?;
    fs::write(root.join("added"), b"added")?;
    fs::write(root.join("modified"), b"changed")?;
    fs::remove_file(root.join("deleted"))?;

    let mut watcher = WatcherProcess::start(&db, root_id)?;
    let conn = rfsync_storage::open(&db)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut files: Vec<_> = rfsync_storage::get_files_by_root_id(&conn, root_id)?
            .into_iter()
            .filter(|file| !file.deleted)
            .map(|file| (file.name, file.size))
            .collect();
        files.sort();
        if files == [("added".into(), 5), ("modified".into(), 7)] {
            break;
        }
        if let Some(status) = watcher.0.try_wait()? {
            return Err(format!("watcher exited before reconciliation: {status}").into());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for reconciliation: {files:?}").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn 감시기는_초기_동기화_에러를_반환하고_스냅샷을_보존한다() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("original"), b"old")?;
    let root_id = register_root(&root, &db, rfsync_engine::DeviceId::from_cert(b"test"))?;
    fs::remove_file(root.join("original"))?;
    fs::write(root.join("invalid\\name"), b"new")?;

    let mut watcher = WatcherProcess::start(&db, root_id)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = watcher.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("watcher did not return initial synchronization error".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    assert!(!status.success());
    let mut stderr = String::new();
    watcher
        .0
        .stderr
        .take()
        .ok_or("missing watcher stderr")?
        .read_to_string(&mut stderr)?;
    assert!(
        stderr.contains("InvalidEntryPath"),
        "unexpected watcher error: {stderr}"
    );
    let conn = rfsync_storage::open(&db)?;
    let files = rfsync_storage::get_files_by_root_id(&conn, root_id)?;
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].name.as_str(), files[0].size), ("original", 3));
    Ok(())
}
