use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_engine::{
    create_initial_snapshot, error::EngineError, register_root, sync_path, sync_root,
};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rfsync-snapshot-boundaries-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
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

#[test]
fn sync_path_converts_nested_file_to_folder() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir_all(root.join("docs"))?;
    fs::write(root.join("docs/item"), b"old")?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    fs::remove_file(root.join("docs/item"))?;
    fs::create_dir(root.join("docs/item"))?;
    fs::write(root.join("docs/item/child"), b"new")?;

    sync_path(root_id, &db, Path::new("docs/item"))?;

    let conn = rfsync_storage::open(&db)?;
    assert!(rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "docs", "item")?.is_none());
    let child = rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "docs/item", "child")?
        .ok_or("missing child")?;
    assert_eq!(child.size, 3);
    Ok(())
}

#[test]
fn deleting_nested_folder_preserves_similar_sibling() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir_all(root.join("docs/item/nested"))?;
    fs::create_dir_all(root.join("docs/item-old"))?;
    fs::write(root.join("docs/item/nested/file"), b"old")?;
    fs::write(root.join("docs/item-old/keep"), b"keep")?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    fs::remove_dir_all(root.join("docs/item"))?;

    sync_path(root_id, &db, Path::new("docs/item"))?;

    let conn = rfsync_storage::open(&db)?;
    assert!(rfsync_storage::get_folders_with_under_path(&conn, root_id, "docs/item")?.is_empty());
    assert!(
        rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "docs/item-old", "keep")?
            .is_some()
    );
    Ok(())
}

#[test]
fn sync_path_does_not_modify_other_root() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let first = dir.0.join("first");
    let second = dir.0.join("second");
    let db = dir.0.join("db");
    for root in [&first, &second] {
        fs::create_dir_all(root.join("docs"))?;
        fs::write(root.join("docs/file"), b"original")?;
    }
    let first_id = register_root(&first, &db)?;
    let second_id = register_root(&second, &db)?;
    create_initial_snapshot(first_id, &db)?;
    create_initial_snapshot(second_id, &db)?;
    fs::remove_dir_all(first.join("docs"))?;

    sync_path(first_id, &db, Path::new("docs"))?;

    let conn = rfsync_storage::open(&db)?;
    assert!(rfsync_storage::get_folders_with_under_path(&conn, first_id, "docs")?.is_empty());
    let other = rfsync_storage::get_file_by_folder_and_name(&conn, second_id, "docs", "file")?
        .ok_or("other root changed")?;
    assert_eq!(other.size, 8);
    Ok(())
}

#[test]
fn repeated_folder_and_deleted_path_syncs_preserve_snapshot() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir_all(root.join("docs"))?;
    fs::write(root.join("docs/file"), b"same")?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    let conn = rfsync_storage::open(&db)?;
    let before = rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "docs", "file")?
        .ok_or("missing file")?;

    sync_path(root_id, &db, Path::new("docs"))?;
    sync_path(root_id, &db, Path::new("docs"))?;
    let after = rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "docs", "file")?
        .ok_or("missing file")?;
    assert_eq!((after.id, &after.hash), (before.id, &before.hash));
    fs::remove_dir_all(root.join("docs"))?;
    sync_path(root_id, &db, Path::new("docs"))?;
    sync_path(root_id, &db, Path::new("docs"))?;
    assert!(rfsync_storage::get_folders_with_under_path(&conn, root_id, "docs")?.is_empty());
    Ok(())
}

#[test]
fn missing_parent_folder_returns_error_without_changing_snapshot() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir(&root)?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    fs::create_dir(root.join("new"))?;
    fs::write(root.join("new/file"), b"data")?;

    assert!(
        matches!(sync_path(root_id, &db, Path::new("new/file")), Err(EngineError::ParentFolderNotFound(path)) if path == Path::new("new"))
    );
    let conn = rfsync_storage::open(&db)?;
    assert!(rfsync_storage::get_folder_by_path(&conn, root_id, "new")?.is_none());
    assert!(rfsync_storage::get_files_by_root_id(&conn, root_id)?.is_empty());
    Ok(())
}

#[test]
fn sync_path_saves_scanned_file_metadata() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir(&root)?;
    fs::write(root.join("file"), b"original")?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    fs::write(root.join("file"), b"modified")?;

    sync_path(root_id, &db, Path::new("file"))?;

    let entry = rfsync_fs::scan_path(&root, Path::new("file"))?.ok_or("missing scanned file")?;
    let rfsync_fs::FsEntryKind::File {
        size,
        hash,
        modified,
    } = entry.kind
    else {
        return Err("expected file".into());
    };
    let modified = modified.duration_since(UNIX_EPOCH)?;
    let conn = rfsync_storage::open(&db)?;
    let file = rfsync_storage::get_file_by_folder_and_name(&conn, root_id, "", "file")?
        .ok_or("missing saved file")?;
    assert_eq!(u64::try_from(file.size)?, size);
    assert_eq!(file.hash, hash);
    assert_eq!(u64::try_from(file.modified_secs)?, modified.as_secs());
    assert_eq!(file.modified_nanos, modified.subsec_nanos());
    Ok(())
}

#[test]
fn sync_rejects_missing_root_and_invalid_paths() -> Result<(), Box<dyn Error>> {
    let dir = TestDir::new()?;
    let root = dir.0.join("root");
    let db = dir.0.join("db");
    fs::create_dir(&root)?;
    let root_id = register_root(&root, &db)?;
    create_initial_snapshot(root_id, &db)?;
    assert!(matches!(
        sync_root(-1, &db),
        Err(EngineError::RootNotFound(-1))
    ));
    assert!(matches!(
        sync_path(-1, &db, Path::new("file")),
        Err(EngineError::RootNotFound(-1))
    ));
    for path in [
        Path::new(""),
        Path::new("../outside"),
        Path::new("/absolute"),
    ] {
        assert!(matches!(
            sync_path(root_id, &db, path),
            Err(EngineError::Fs(_))
        ));
    }
    Ok(())
}
