use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_storage::{Folder, Root, error::StorageError};
use rusqlite::{Connection, Transaction};

use crate::{
    diff::{SnapshotDiff, calc_diff},
    error::EngineError,
};

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
pub(crate) fn sync_root(conn: &mut Connection, root: &Root) -> Result<(), EngineError> {
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

pub(crate) fn sync_path(
    conn: &mut Connection,
    root: &Root,
    target: &Path,
) -> Result<(), EngineError> {
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

fn sync_directory(
    tx: &Transaction<'_>,
    root_id: i64,
    root_path: &Path,
    target: &Path,
    target_path: &str,
    entry: FsEntry,
) -> Result<(), EngineError> {
    let mut entries = rfsync_fs::scan(&root_path.join(target))?;

    // 하위 폴더 평면화
    for entry in &mut entries {
        entry.path = target.join(&entry.path);
    }
    entries.push(entry);

    // 해당 폴더가 이전 파일이였다면 제거
    let (parent_path, name) = split_parent_and_name(target_path);

    if let Some(file) = rfsync_storage::get_file_by_folder_and_name(tx, root_id, parent_path, name)?
        && !rfsync_storage::delete_file_by_id(tx, file.id)?
    {
        return Err(EngineError::FileNotFound(target.to_path_buf()));
    }

    let folders = rfsync_storage::get_folders_with_under_path(tx, root_id, target_path)?;
    let folder_ids: Vec<i64> = folders.iter().map(|folder| folder.id).collect();
    let files = rfsync_storage::get_files_in_folder_ids(tx, &folder_ids)?;

    let diff = calc_diff(&entries, &folders, &files)?;
    apply_diff(tx, root_id, &folders, diff)?;
    Ok(())
}

fn sync_file(
    tx: &Transaction<'_>,
    root_id: i64,
    target: &Path,
    target_path: &str,
    entry: FsEntry,
) -> Result<(), EngineError> {
    let (folder_path, file_name) = split_parent_and_name(target_path);

    let parent =
        rfsync_storage::get_folder_by_path(tx, root_id, folder_path)?.ok_or_else(|| {
            EngineError::ParentFolderNotFound(
                target.parent().unwrap_or(Path::new("")).to_path_buf(),
            )
        })?;

    // 이전에 같은 경로가 폴더였다면 하위 폴더까지 제거
    remove_folder_tree(tx, root_id, target_path)?;

    let existing =
        rfsync_storage::get_file_by_folder_id_and_name(tx, root_id, parent.id, file_name)?;

    let folder_ids = HashMap::from([(
        target.parent().unwrap_or(Path::new("")).to_path_buf(),
        parent.id,
    )]);

    match existing {
        Some(file) => {
            let updated = to_storage_file(&entry, file.id, &folder_ids)?;
            if !rfsync_storage::update_file(tx, updated)? {
                return Err(EngineError::FileNotFound(target.to_path_buf()));
            }
        }
        None => {
            let new_file = to_storage_file(&entry, 0, &folder_ids)?;
            rfsync_storage::insert_file(tx, &new_file)?;
        }
    }

    Ok(())
}

fn remove_path(tx: &Transaction<'_>, root_id: i64, target_path: &str) -> Result<(), EngineError> {
    remove_folder_tree(tx, root_id, target_path)?;

    let (folder_path, file_name) = split_parent_and_name(target_path);

    if let Some(file) =
        rfsync_storage::get_file_by_folder_and_name(tx, root_id, folder_path, file_name)?
    {
        rfsync_storage::delete_file_by_id(tx, file.id)?;
    }

    Ok(())
}

fn apply_diff(
    tx: &Transaction<'_>,
    root_id: i64,
    folders: &[Folder],
    diff: SnapshotDiff<'_>,
) -> Result<(), EngineError> {
    let mut folder_ids: HashMap<PathBuf, i64> = folders
        .iter()
        .map(|folder| (PathBuf::from(&folder.relative_path), folder.id))
        .collect();

    for (remove_file_id, path) in diff.removed_files {
        if !rfsync_storage::delete_file_by_id(tx, remove_file_id)? {
            return Err(EngineError::FileNotFound(path.clone()));
        }
    }

    for (remove_folder_id, path) in diff.removed_folders {
        if !rfsync_storage::delete_folder_by_id(tx, remove_folder_id)? {
            return Err(EngineError::FolderNotFound(path.clone()));
        }

        folder_ids.remove(&path);
    }

    for added_folder_path in diff.added_folders {
        let relative_path = portable_path(&added_folder_path)?;
        let folder_id = rfsync_storage::insert_folder(tx, root_id, &relative_path)?;
        folder_ids.insert(added_folder_path.clone(), folder_id);
    }

    for added_entry in diff.added_files {
        let file = to_storage_file(added_entry, 0, &folder_ids)?;
        rfsync_storage::insert_file(tx, &file)?;
    }

    for (modified_file_id, modified_entry) in diff.modified_files {
        let file = to_storage_file(modified_entry, modified_file_id, &folder_ids)?;
        if !rfsync_storage::update_file(tx, file)? {
            return Err(EngineError::FileNotFound(modified_entry.path.clone()));
        }
    }

    Ok(())
}

fn remove_folder_tree(
    tx: &Transaction<'_>,
    root_id: i64,
    target_path: &str,
) -> Result<(), EngineError> {
    let folders = rfsync_storage::get_folders_with_under_path(tx, root_id, target_path)?;

    for folder in folders {
        rfsync_storage::delete_folder_by_id(tx, folder.id)?;
    }

    Ok(())
}

/// UNIX 스타일로 경로 변환
/// error: UTF-8 표현 불가 및 '\' 포함
fn portable_path(path: &Path) -> Result<String, EngineError> {
    let mut parts = Vec::new();

    for compoenct in path.components() {
        let Component::Normal(name) = compoenct else {
            return Err(EngineError::InvalidEntryPath(path.to_path_buf()));
        };

        let name = name
            .to_str()
            .ok_or_else(|| EngineError::InvalidEntryPath(path.to_path_buf()))?;

        if name.contains('\\') {
            return Err(EngineError::InvalidEntryPath(path.to_path_buf()));
        }

        parts.push(name);
    }

    if parts.is_empty() {
        return Err(EngineError::InvalidEntryPath(path.to_path_buf()));
    }

    Ok(parts.join("/"))
}

fn to_storage_file(
    entry: &FsEntry,
    id: i64,
    folder_ids: &HashMap<PathBuf, i64>,
) -> Result<rfsync_storage::File, EngineError> {
    let FsEntryKind::File {
        size,
        hash,
        modified,
    } = &entry.kind
    else {
        return Err(EngineError::InvalidEntryPath(entry.path.clone()));
    };

    let parent = entry
        .path
        .parent()
        .ok_or_else(|| EngineError::InvalidEntryPath(entry.path.clone()))?;

    let folder_id = *folder_ids
        .get(parent)
        .ok_or_else(|| EngineError::ParentFolderNotFound(parent.to_path_buf()))?;

    let relative_path = portable_path(&entry.path)?;
    let name = relative_path.rsplit('/').next().unwrap_or(&relative_path);
    let modified = modified.duration_since(std::time::UNIX_EPOCH)?;

    Ok(rfsync_storage::File {
        id,
        folder_id,
        name: name.to_owned(),
        size: i64::try_from(*size)?,
        hash: hash.clone(),
        modified_secs: i64::try_from(modified.as_secs())?,
        modified_nanos: modified.subsec_nanos(),
    })
}

fn split_parent_and_name(target_path: &str) -> (&str, &str) {
    target_path.rsplit_once('/').unwrap_or(("", target_path))
}

#[cfg(test)]
mod tests;
