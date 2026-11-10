use std::{
    error::Error,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_engine::{diff::diff_snapshot, error::EngineError};

fn test_dir() -> Result<PathBuf, Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!(
        "rfsync-diff-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

#[test]
fn reports_no_changes_after_initial_snapshot() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db = dir.join("snapshot.db");
    fs::create_dir_all(root.join("nested/empty"))?;
    fs::write(root.join("nested/file"), b"contents")?;
    let root_id = rfsync_engine::register_root(&root, &db)?;
    rfsync_engine::create_initial_snapshot(root_id, &db)?;

    let diff = diff_snapshot(root_id, &db)?;

    assert!(diff.added_folders.is_empty());
    assert!(diff.removed_folders.is_empty());
    assert!(diff.added_files.is_empty());
    assert!(diff.modified_files.is_empty());
    assert!(diff.removed_files.is_empty());
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn reports_changes_and_excludes_files_in_removed_folders() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let root = dir.join("root");
    let db = dir.join("snapshot.db");
    fs::create_dir_all(root.join("removed/nested"))?;
    fs::write(root.join("removed/nested/file"), b"old")?;
    fs::write(root.join("modified"), b"old")?;
    fs::write(root.join("deleted"), b"old")?;
    fs::write(root.join("unchanged"), b"same")?;
    let root_id = rfsync_engine::register_root(&root, &db)?;
    rfsync_engine::create_initial_snapshot(root_id, &db)?;
    let conn = rfsync_storage::open(&db)?;
    let folders = rfsync_storage::get_folders_by_root_id(&conn, root_id)?;
    let files = rfsync_storage::get_files_by_root_id(&conn, root_id)?;
    let folder_id = |path: &str| {
        folders
            .iter()
            .find(|f| f.relative_path == path)
            .map(|f| f.id)
            .ok_or("missing folder")
    };
    let file_id = |name: &str| {
        files
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.id)
            .ok_or("missing file")
    };
    fs::remove_dir_all(root.join("removed"))?;
    fs::remove_file(root.join("deleted"))?;
    fs::write(root.join("modified"), b"new contents")?;
    fs::create_dir(root.join("added"))?;
    fs::write(root.join("added/file"), b"new")?;

    let diff = diff_snapshot(root_id, &db)?;

    assert_eq!(diff.added_folders, [PathBuf::from("added")]);
    assert_eq!(diff.added_files, [PathBuf::from("added/file")]);
    assert_eq!(diff.removed_folders.len(), 2);
    assert_eq!(
        diff.removed_folders.get(&folder_id("removed")?),
        Some(&PathBuf::from("removed"))
    );
    assert_eq!(
        diff.removed_folders.get(&folder_id("removed/nested")?),
        Some(&PathBuf::from("removed/nested"))
    );
    assert_eq!(diff.modified_files.len(), 1);
    assert_eq!(
        diff.modified_files.get(&file_id("modified")?),
        Some(&PathBuf::from("modified"))
    );
    assert_eq!(diff.removed_files.len(), 1);
    assert_eq!(
        diff.removed_files.get(&file_id("deleted")?),
        Some(&PathBuf::from("deleted"))
    );
    drop(conn);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rejects_missing_root() -> Result<(), Box<dyn Error>> {
    let dir = test_dir()?;
    let result = diff_snapshot(42, &dir.join("snapshot.db"));
    assert!(matches!(result, Err(EngineError::RootNotFound(42))));
    fs::remove_dir_all(dir)?;
    Ok(())
}
