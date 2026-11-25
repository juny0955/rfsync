use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_protocol::DeviceId;
use rfsync_storage::{File, Folder, SyncPending};

use crate::error::EngineError;

use super::{
    diff::{SnapshotDiff, calc_diff},
    entry::{portable_path, to_local_storage_file},
    version::deleted_file_version,
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

    for entry in &mut entries {
        entry.path = target.join(&entry.path);
    }
    entries.push(entry);

    let (parent_path, name) = split_parent_and_name(target_path);

    if let Some(file) = rfsync_storage::get_file_by_folder_and_name(tx, root_id, parent_path, name)?
        && !mark_deleted(tx, root_id, target_path, &file, me)?
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
    mark_folder_tree_deleted(tx, root_id, target_path, me)?;

    let existing =
        rfsync_storage::get_file_by_folder_id_and_name(tx, root_id, parent.id, file_name)?;
    // 상대에게서 받은 파일이면 그 기록을 꺼낸다. 이 파일의 version은 받은 것을 따른다.
    let pending = take_matching_pending(tx, root_id, target_path, observed_hash(&entry), false)?;

    let folder_ids = HashMap::from([(
        target.parent().unwrap_or(Path::new("")).to_path_buf(),
        parent.id,
    )]);

    match existing {
        Some(file) => {
            let updated = to_local_storage_file(
                &entry,
                file.id,
                &folder_ids,
                Some(&file),
                pending.as_ref(),
                me,
            )?;
            if !rfsync_storage::update_file(tx, updated)? {
                return Err(EngineError::FileNotFound(target.to_path_buf()));
            }
        }
        None => {
            let new_file =
                to_local_storage_file(&entry, 0, &folder_ids, None, pending.as_ref(), me)?;
            rfsync_storage::insert_file(tx, &new_file)?;
        }
    }

    Ok(())
}

pub fn remove_path(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    target_path: &str,
    me: DeviceId,
) -> Result<(), EngineError> {
    mark_folder_tree_deleted(tx, root_id, target_path, me)?;

    let (folder_path, file_name) = split_parent_and_name(target_path);

    if let Some(file) =
        rfsync_storage::get_file_by_folder_and_name(tx, root_id, folder_path, file_name)?
    {
        mark_deleted(tx, root_id, target_path, &file, me)?;
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
        let file = previous
            .get(&remove_file_id)
            .copied()
            .ok_or_else(|| EngineError::FileNotFound(path.clone()))?;
        if !mark_deleted(tx, root_id, &portable_path(&path)?, file, me)? {
            return Err(EngineError::FileNotFound(path.clone()));
        }
    }

    for added_folder_path in diff.added_folders {
        let relative_path = portable_path(&added_folder_path)?;
        let folder_id = rfsync_storage::insert_folder(tx, root_id, &relative_path)?;
        folder_ids.insert(added_folder_path.clone(), folder_id);
    }

    for added_entry in diff.added_files {
        let pending = take_matching_pending(
            tx,
            root_id,
            &portable_path(&added_entry.path)?,
            observed_hash(added_entry),
            false,
        )?;
        let file = to_local_storage_file(added_entry, 0, &folder_ids, None, pending.as_ref(), me)?;
        rfsync_storage::insert_file(tx, &file)?;
    }

    for (modified_file_id, modified_entry) in diff.modified_files {
        let before = previous
            .get(&modified_file_id)
            .copied()
            .ok_or_else(|| EngineError::FileNotFound(modified_entry.path.clone()))?;
        let pending = take_matching_pending(
            tx,
            root_id,
            &portable_path(&modified_entry.path)?,
            observed_hash(modified_entry),
            false,
        )?;
        let file = to_local_storage_file(
            modified_entry,
            modified_file_id,
            &folder_ids,
            Some(before),
            pending.as_ref(),
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
    me: DeviceId,
) -> Result<(), EngineError> {
    let folders = rfsync_storage::get_folders_with_under_path(tx, root_id, target_path)?;
    let folder_ids: Vec<i64> = folders.iter().map(|folder| folder.id).collect();

    let folder_paths: HashMap<i64, &str> = folders
        .iter()
        .map(|folder| (folder.id, folder.relative_path.as_str()))
        .collect();

    for file in rfsync_storage::get_files_in_folder_ids(tx, &folder_ids)? {
        let folder_path = folder_paths.get(&file.folder_id).copied().unwrap_or("");
        let path = if folder_path.is_empty() {
            file.name.clone()
        } else {
            format!("{folder_path}/{}", file.name)
        };
        mark_deleted(tx, root_id, &path, &file, me)?;
    }

    Ok(())
}

/// 파일을 삭제 표시하고 version을 올린다. 이미 삭제 표시된 파일은 건드리지 않는다.
/// 상대가 지우라고 해서 지운 것이면(`path`의 sync_pending 기록이 삭제) 상대의 version을 그대로 쓴다.
///
/// 이미 삭제된 파일의 version을 또 올리면 폴더 이벤트가 올 때마다 삭제 기록이 앞서 나간다.
/// 반환값은 해당 행이 있었는지다.
fn mark_deleted(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    path: &str,
    file: &File,
    me: DeviceId,
) -> Result<bool, EngineError> {
    // 상대가 지워서 지운 것이면 그 기록을 꺼낸다. 이미 삭제 표시된 파일이어도 맞는 기록은 지운다.
    let pending = take_matching_pending(tx, root_id, path, &file.hash, true)?;
    if file.deleted {
        return Ok(true);
    }
    let version = deleted_file_version(file, pending.as_ref(), me)?;
    Ok(rfsync_storage::mark_file_deleted(tx, file.id, &version)?)
}

/// 디스크에서 본 파일의 해시. 파일이 아니면 빈 문자열이라 어떤 기록과도 맞지 않는다.
fn observed_hash(entry: &FsEntry) -> &str {
    match &entry.kind {
        FsEntryKind::File { hash, .. } => hash,
        _ => "",
    }
}

/// 경로의 `sync_pending` 기록이 지금 관측한 상태(`hash`, `deleted`)와 같을 때만 꺼낸다.
///
/// 받는 도중에 같은 경로의 다른 이벤트(받기 전 옛 파일, 내가 지우거나 고친 것)가 먼저 처리되면 기록은
/// 아직 쓰일 때가 아니다. 그때 꺼내 버리면 정작 받은 파일의 이벤트에서 상대의 version을 잃고 내 수정으로 기록된다.
/// 맞지 않으면 기록을 그대로 두고 `None`을 돌려준다. 받기에 실패하면 받는 쪽이 지운다.
fn take_matching_pending(
    tx: &rfsync_storage::Transaction<'_>,
    root_id: i64,
    path: &str,
    hash: &str,
    deleted: bool,
) -> Result<Option<SyncPending>, EngineError> {
    let Some(pending) = rfsync_storage::take_sync_pending(tx, root_id, path)? else {
        return Ok(None);
    };
    if pending.hash == hash && pending.deleted == deleted {
        return Ok(Some(pending));
    }
    rfsync_storage::put_sync_pending(tx, root_id, &pending)?;
    Ok(None)
}

fn split_parent_and_name(target_path: &str) -> (&str, &str) {
    target_path.rsplit_once('/').unwrap_or(("", target_path))
}
