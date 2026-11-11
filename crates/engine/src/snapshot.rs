use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_storage::error::StorageError;

use crate::{diff::calc_diff, error::EngineError};

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
pub fn sync_root(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    if !root.has_snapshot {
        return Err(EngineError::NotInitializedRoot(root_id));
    }

    let entries = rfsync_fs::scan(Path::new(&root.local_path))?;

    // === START TRANSACTION ===
    let tx = conn.transaction().map_err(StorageError::from)?;
    let folders = rfsync_storage::get_folders_by_root_id(&tx, root_id)?;
    let files = rfsync_storage::get_files_by_root_id(&tx, root_id)?;

    let diff = calc_diff(&entries, &folders, &files)?;

    let mut folder_ids: HashMap<PathBuf, i64> = folders
        .iter()
        .map(|folder| (PathBuf::from(&folder.relative_path), folder.id))
        .collect();

    for (remove_file_id, path) in diff.removed_files {
        if !rfsync_storage::delete_file_by_id(&tx, remove_file_id)? {
            return Err(EngineError::FileNotFound(path.clone()));
        }
    }

    for (remove_folder_id, path) in diff.removed_folders {
        if !rfsync_storage::delete_folder_by_id(&tx, remove_folder_id)? {
            return Err(EngineError::FolderNotFound(path.clone()));
        }

        folder_ids.remove(&path);
    }

    for added_folder_path in diff.added_folders {
        let relative_path = portable_path(&added_folder_path)?;
        let folder_id = rfsync_storage::insert_folder(&tx, root_id, &relative_path)?;
        folder_ids.insert(added_folder_path.clone(), folder_id);
    }

    for added_entry in diff.added_files {
        let file = to_storage_file(added_entry, 0, &folder_ids)?;
        rfsync_storage::insert_file(&tx, &file)?;
    }

    for (modified_file_id, modified_entry) in diff.modified_files {
        let file = to_storage_file(modified_entry, modified_file_id, &folder_ids)?;
        if !rfsync_storage::update_file(&tx, file)? {
            return Err(EngineError::FileNotFound(modified_entry.path.clone()));
        }
    }

    tx.commit().map_err(StorageError::from)?;
    // === END TRANSACTION ===

    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_entry_paths() {
        for path in ["", "..", "invalid\\name"] {
            assert!(
                matches!(portable_path(Path::new(path)), Err(EngineError::InvalidEntryPath(p)) if p == Path::new(path))
            );
        }
    }
}
