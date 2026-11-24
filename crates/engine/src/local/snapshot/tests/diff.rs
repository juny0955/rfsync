use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_fs::FsEntryKind;

use crate::{error::EngineError, register_root};

use super::super::diff::calc_diff;

fn test_dir() -> Result<PathBuf, Box<dyn Error>> {
    // 시각만으로는 병렬로 도는 테스트가 같은 이름을 만들 수 있어 번호표를 섞는다.
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rfsync-diff-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
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
    let root_id = register_root(&root, &db)?;

    let conn = rfsync_storage::open(&db)?;
    let folders = rfsync_storage::get_folders_by_root_id(&conn, root_id)?;
    let files = rfsync_storage::get_files_by_root_id(&conn, root_id)?;
    let entries = rfsync_fs::scan(&root)?;
    let diff = calc_diff(&entries, &folders, &files)?;

    assert!(diff.added_folders.is_empty());
    assert!(diff.removed_folders.is_empty());
    assert!(diff.added_files.is_empty());
    assert!(diff.modified_files.is_empty());
    assert!(diff.removed_files.is_empty());
    drop(conn);
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
    let root_id = register_root(&root, &db)?;
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

    let entries = rfsync_fs::scan(&root)?;
    let diff = calc_diff(&entries, &folders, &files)?;

    assert_eq!(diff.added_folders, [PathBuf::from("added")]);
    assert_eq!(diff.added_files.len(), 1);
    assert_eq!(diff.added_files[0].path, PathBuf::from("added/file"));
    assert!(matches!(
        &diff.added_files[0].kind,
        FsEntryKind::File { size: 3, .. }
    ));
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
    let modified = diff
        .modified_files
        .get(&file_id("modified")?)
        .ok_or("missing modified file")?;
    assert_eq!(modified.path, PathBuf::from("modified"));
    assert!(matches!(&modified.kind, FsEntryKind::File { size: 12, .. }));
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
fn rejects_file_with_missing_parent_folder() -> Result<(), Box<dyn Error>> {
    let file = rfsync_storage::File {
        id: 1,
        folder_id: 42,
        name: "file".into(),
        size: 0,
        hash: "a".repeat(64),
        modified_secs: 0,
        modified_nanos: 0,
    };
    let result = calc_diff(&[], &[], &[file]);
    assert!(
        matches!(result, Err(EngineError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
    );
    Ok(())
}
