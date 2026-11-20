use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use rfsync_storage::{Root, error::StorageError};
use rusqlite::Connection;

use crate::{
    error::EngineError,
    snapshot::{
        diff::calc_diff,
        sync::{apply_diff, remove_path, sync_directory, sync_file},
    },
};

use self::entry::{portable_path, to_storage_file};

mod diff;
mod entry;
mod sync;

pub fn create_initial_snapshot(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;

    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    if root.has_snapshot {
        return Err(EngineError::SnapshotAlreadyExists(root_id));
    }

    let entries = rfsync_fs::scan(Path::new(&root.local_path))?;

    let mut folder_ids = HashMap::<PathBuf, i64>::new();

    // === START TRANSACTION ===
    let tx = conn.transaction().map_err(StorageError::from)?;
    let root_folder_id = rfsync_storage::insert_folder(&tx, root_id, "")?;
    folder_ids.insert(PathBuf::new(), root_folder_id);

    for entry in &entries {
        if entry.is_dir() {
            let relative_path = portable_path(&entry.path)?;
            let folder_id = rfsync_storage::insert_folder(&tx, root_id, &relative_path)?;
            folder_ids.insert(entry.path.clone(), folder_id);
        }
    }

    for entry in entries.iter().filter(|entry| !entry.is_dir()) {
        let file = to_storage_file(entry, 0, &folder_ids)?;
        rfsync_storage::insert_file(&tx, &file)?;
    }

    if !rfsync_storage::mark_root_has_snapshot(&tx, root_id)? {
        return Err(EngineError::RootNotFound(root_id));
    }

    tx.commit().map_err(StorageError::from)?;
    // === END TRANSACTION ===

    Ok(())
}

/// root 전체 동기화
/// 1. 파일 삭제
/// 2. 폴더 삭제
/// 3. 폴더 추가
/// 4. 파일 추가
/// 5. 파일 수정
pub fn sync_root(conn: &mut Connection, root: &Root) -> Result<(), EngineError> {
    let entries = rfsync_fs::scan(Path::new(&root.local_path))?;

    // === START TRANSACTION ===
    let tx = conn.transaction().map_err(StorageError::from)?;
    let folders = rfsync_storage::get_folders_by_root_id(&tx, root.id)?;
    let files = rfsync_storage::get_files_by_root_id(&tx, root.id)?;

    let diff = calc_diff(&entries, &folders, &files)?;
    apply_diff(&tx, root.id, &folders, diff)?;
    tx.commit().map_err(StorageError::from)?;
    // === END TRANSACTION ===

    Ok(())
}

pub fn sync_path(conn: &mut Connection, root: &Root, target: &Path) -> Result<(), EngineError> {
    let root_path = Path::new(&root.local_path);
    let entry = rfsync_fs::scan_path(root_path, target)?;

    let target_path = portable_path(target)?;
    // === START TRANSACTION ===
    let tx = conn.transaction().map_err(StorageError::from)?;

    match entry {
        // 폴더
        Some(entry) if entry.is_dir() => {
            sync_directory(&tx, root.id, root_path, target, &target_path, entry)?
        }
        // 파일
        Some(entry) => sync_file(&tx, root.id, target, &target_path, entry)?,
        // 스캔 결과 없음 -> 제거
        None => remove_path(&tx, root.id, &target_path)?,
    }

    tx.commit().map_err(StorageError::from)?;
    // === END TRANSACTION ===

    Ok(())
}

#[cfg(test)]
mod tests;
