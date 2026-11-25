use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{error::EngineError, register_root};

use super::create_initial_snapshot_for_test;

fn test_dir() -> Result<PathBuf, Box<dyn Error>> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rfsync-snapshot-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

#[test]
fn 파일과_빈_폴더를_저장하고_심볼릭_링크는_무시한다() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::create_dir_all(root.join("photos/empty"))?;
    fs::write(root.join("empty.txt"), b"")?;
    fs::write(root.join("photos/photo.txt"), b"photo")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&root, root.join("link"))?;
    let root_id = register_root(&root, &db_path, crate::DeviceId::from_cert(b"test"))?;

    let conn = rfsync_storage::open(&db_path)?;
    let root_info = rfsync_storage::get_root(&conn, root_id)?.ok_or("missing root")?;
    assert!(root_info.has_snapshot);
    let mut folders = conn
        .prepare("SELECT relative_path FROM folders WHERE root_id = ?1 ORDER BY relative_path")?;
    let paths = folders
        .query_map([root_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(paths, ["", "photos", "photos/empty"]);
    let mut files = conn.prepare("SELECT folders.relative_path, files.name, files.fsize, files.hash FROM files JOIN folders ON folders.id = files.folder_id WHERE folders.root_id = ?1 ORDER BY folders.relative_path")?;
    let entries = files
        .query_map([root_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0],
        (
            String::new(),
            "empty.txt".into(),
            0,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262".into()
        )
    );
    assert_eq!(
        (&entries[1].0, &entries[1].1, entries[1].2),
        (&"photos".to_owned(), &"photo.txt".to_owned(), 5)
    );
    drop(files);
    drop(folders);
    drop(conn);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn 두_번째_스냅샷은_거부하고_원본을_보존한다() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("first"), b"first")?;
    let root_id = register_root(&root, &db_path, crate::DeviceId::from_cert(b"test"))?;
    fs::write(root.join("second"), b"second")?;

    let error = create_initial_snapshot_for_test(root_id, &db_path)
        .expect_err("duplicate snapshot must fail");

    assert!(matches!(error, EngineError::SnapshotAlreadyExists(id) if id == root_id));
    let conn = rfsync_storage::open(&db_path)?;
    let names: Vec<String> = conn
        .prepare("SELECT name FROM files")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(names, ["first"]);
    drop(conn);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn 없는_루트의_스냅샷은_거부한다() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let result = create_initial_snapshot_for_test(42, &dir.join("snapshot.db"));
    assert!(matches!(result, Err(EngineError::RootNotFound(42))));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn 경로를_표현할_수_없으면_스냅샷을_롤백한다() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("invalid\\name"), b"test")?;
    let result = register_root(&root, &db_path, crate::DeviceId::from_cert(b"test"));

    assert!(
        matches!(result, Err(EngineError::InvalidEntryPath(path)) if path == std::path::Path::new("invalid\\name"))
    );
    let conn = rfsync_storage::open(&db_path)?;
    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root.to_str().ok_or("non-UTF-8 test path")?;
    let saved_root = rfsync_storage::get_root_by_path(&conn, local_path)?.ok_or("missing root")?;
    assert!(!saved_root.has_snapshot);
    let folders: i64 = conn.query_row(
        "SELECT count(*) FROM folders WHERE root_id = ?1",
        [saved_root.id],
        |row| row.get(0),
    )?;
    assert_eq!(folders, 0);
    drop(conn);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn 초기_스냅샷의_파일은_내_카운터_1로_시작한다() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir_all(root.join("sub"))?;
    fs::write(root.join("a.txt"), b"a")?;
    fs::write(root.join("sub/b.txt"), b"b")?;
    let me = crate::DeviceId::from_cert(b"me");
    let root_id = register_root(&root, &db_path, me)?;

    let conn = rfsync_storage::open(&db_path)?;
    let files = rfsync_storage::get_files_by_root_id(&conn, root_id)?;

    assert_eq!(files.len(), 2);
    let mut expected = rfsync_protocol::VersionVector::default();
    expected.bump(me);
    for file in files {
        assert_eq!(
            rfsync_protocol::VersionVector::from_bytes(&file.version)?,
            expected
        );
    }
    fs::remove_dir_all(dir)?;
    Ok(())
}
