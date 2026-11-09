use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_dir() -> Result<PathBuf, Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!(
        "rfsync-snapshot-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

#[test]
fn saves_files_empty_folders_and_ignores_symlinks() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::create_dir_all(root.join("photos/empty"))?;
    fs::write(root.join("empty.txt"), b"")?;
    fs::write(root.join("photos/photo.txt"), b"photo")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&root, root.join("link"))?;
    let root_id = rfsync_core::register_root(&root, &db_path)?;

    rfsync_core::create_initial_snapshot(root_id, &db_path)?;

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
fn rejects_second_snapshot_and_preserves_original() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("first"), b"first")?;
    let root_id = rfsync_core::register_root(&root, &db_path)?;
    rfsync_core::create_initial_snapshot(root_id, &db_path)?;
    fs::write(root.join("second"), b"second")?;

    let error = rfsync_core::create_initial_snapshot(root_id, &db_path)
        .expect_err("duplicate snapshot must fail");

    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::AlreadyExists)
    );
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

#[cfg(unix)]
#[test]
fn rolls_back_snapshot_when_path_cannot_be_represented() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db_path = dir.join("snapshot.db");
    fs::create_dir(&root)?;
    fs::write(root.join("invalid\\name"), b"test")?;
    let root_id = rfsync_core::register_root(&root, &db_path)?;

    let result = rfsync_core::create_initial_snapshot(root_id, &db_path);

    assert!(result.is_err());
    let conn = rfsync_storage::open(&db_path)?;
    assert!(
        !rfsync_storage::get_root(&conn, root_id)?
            .ok_or("missing root")?
            .has_snapshot
    );
    let folders: i64 = conn.query_row(
        "SELECT count(*) FROM folders WHERE root_id = ?1",
        [root_id],
        |row| row.get(0),
    )?;
    assert_eq!(folders, 0);
    drop(conn);
    fs::remove_dir_all(dir)?;
    Ok(())
}
