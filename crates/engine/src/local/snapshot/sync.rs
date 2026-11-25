use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use rfsync_fs::FsEntry;
use rfsync_protocol::DeviceId;
use rfsync_storage::{File, Folder};

use crate::error::EngineError;

use super::{
    diff::{SnapshotDiff, calc_diff},
    entry::{portable_path, to_local_storage_file},
};

pub fn sync_directory(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    root_path: &Path,
    target: &Path,
    target_path: &str,
    entry: FsEntry,
    me: DeviceId,
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
        && !rfsync_storage::mark_file_deleted(tx, file.id)?
    {
        return Err(EngineError::FileNotFound(target.to_path_buf()));
    }

    let folders = rfsync_storage::get_folders_with_under_path(tx, root_id, target_path)?;
    let folder_ids: Vec<i64> = folders.iter().map(|folder| folder.id).collect();
    let files = rfsync_storage::get_files_in_folder_ids(tx, &folder_ids)?;

    let diff = calc_diff(&entries, &folders, &files)?;
    apply_diff(tx, root_id, &folders, &files, diff, me)?;
    Ok(())
}

pub fn sync_file(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    target: &Path,
    target_path: &str,
    entry: FsEntry,
    me: DeviceId,
) -> Result<(), EngineError> {
    let (folder_path, file_name) = split_parent_and_name(target_path);

    let parent =
        rfsync_storage::get_folder_by_path(tx, root_id, folder_path)?.ok_or_else(|| {
            EngineError::ParentFolderNotFound(
                target.parent().unwrap_or(Path::new("")).to_path_buf(),
            )
        })?;

    // 이전에 같은 경로가 폴더였다면 아래 파일을 모두 삭제 표시
    mark_folder_tree_deleted(tx, root_id, target_path)?;

    let existing =
        rfsync_storage::get_file_by_folder_id_and_name(tx, root_id, parent.id, file_name)?;

    let folder_ids = HashMap::from([(
        target.parent().unwrap_or(Path::new("")).to_path_buf(),
        parent.id,
    )]);

    match existing {
        Some(file) => {
            let updated = to_local_storage_file(&entry, file.id, &folder_ids, Some(&file), me)?;
            if !rfsync_storage::update_file(tx, updated)? {
                return Err(EngineError::FileNotFound(target.to_path_buf()));
            }
        }
        None => {
            let new_file = to_local_storage_file(&entry, 0, &folder_ids, None, me)?;
            rfsync_storage::insert_file(tx, &new_file)?;
        }
    }

    Ok(())
}

pub fn remove_path(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    target_path: &str,
) -> Result<(), EngineError> {
    mark_folder_tree_deleted(tx, root_id, target_path)?;

    let (folder_path, file_name) = split_parent_and_name(target_path);

    if let Some(file) =
        rfsync_storage::get_file_by_folder_and_name(tx, root_id, folder_path, file_name)?
    {
        rfsync_storage::mark_file_deleted(tx, file.id)?;
    }

    Ok(())
}

pub fn apply_diff(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    folders: &[Folder],
    files: &[File],
    diff: SnapshotDiff<'_>,
    me: DeviceId,
) -> Result<(), EngineError> {
    let previous: HashMap<i64, &File> = files.iter().map(|file| (file.id, file)).collect();
    let mut folder_ids: HashMap<PathBuf, i64> = folders
        .iter()
        .map(|folder| (PathBuf::from(&folder.relative_path), folder.id))
        .collect();

    for (remove_file_id, path) in diff.removed_files {
        if !rfsync_storage::mark_file_deleted(tx, remove_file_id)? {
            return Err(EngineError::FileNotFound(path.clone()));
        }
    }

    for added_folder_path in diff.added_folders {
        let relative_path = portable_path(&added_folder_path)?;
        let folder_id = rfsync_storage::insert_folder(tx, root_id, &relative_path)?;
        folder_ids.insert(added_folder_path.clone(), folder_id);
    }

    for added_entry in diff.added_files {
        let file = to_local_storage_file(added_entry, 0, &folder_ids, None, me)?;
        rfsync_storage::insert_file(tx, &file)?;
    }

    for (modified_file_id, modified_entry) in diff.modified_files {
        let before = previous
            .get(&modified_file_id)
            .copied()
            .ok_or_else(|| EngineError::FileNotFound(modified_entry.path.clone()))?;
        let file = to_local_storage_file(
            modified_entry,
            modified_file_id,
            &folder_ids,
            Some(before),
            me,
        )?;
        if !rfsync_storage::update_file(tx, file)? {
            return Err(EngineError::FileNotFound(modified_entry.path.clone()));
        }
    }

    Ok(())
}

/// 경로 아래 모든 폴더의 파일을 삭제 표시한다. 폴더 행은 남긴다(지우면 CASCADE로 삭제 기록도 사라진다).
fn mark_folder_tree_deleted(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    target_path: &str,
) -> Result<(), EngineError> {
    let folders = rfsync_storage::get_folders_with_under_path(tx, root_id, target_path)?;
    let folder_ids: Vec<i64> = folders.iter().map(|folder| folder.id).collect();

    for file in rfsync_storage::get_files_in_folder_ids(tx, &folder_ids)? {
        rfsync_storage::mark_file_deleted(tx, file.id)?;
    }

    Ok(())
}

fn split_parent_and_name(target_path: &str) -> (&str, &str) {
    target_path.rsplit_once('/').unwrap_or(("", target_path))
}
