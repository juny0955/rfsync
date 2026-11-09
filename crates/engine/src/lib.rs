use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::EntryKind;
use rfsync_storage::error::StorageError;

use crate::error::EngineError;

pub mod error;

pub fn register_root(root: &Path, db_path: &Path) -> Result<i64, EngineError> {
    let conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root
        .to_str()
        .ok_or_else(|| EngineError::InvalidRootPath(absolute_root.clone()))?;

    if rfsync_storage::exists_root_path(&conn, local_path)? {
        return Err(EngineError::RootAlreadyExists(absolute_root));
    }

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;
    Ok(root_id)
}

pub fn create_initial_snapshot(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;

    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    if root.has_snapshot {
        return Err(EngineError::SnapshotAlreadyExists(root_id));
    }

    let result = rfsync_fs::scan(Path::new(&root.local_path))?;

    let mut folder_ids = HashMap::<PathBuf, i64>::new();

    // === START TRANSACTION ===
    let tx = conn.transaction().map_err(StorageError::from)?;
    let root_folder_id = rfsync_storage::insert_folder(&tx, root_id, "")?;
    folder_ids.insert(PathBuf::new(), root_folder_id);

    for entry in &result.entries {
        if entry.is_dir() {
            let relative_path = portable_path(&entry.path)?;
            let folder_id = rfsync_storage::insert_folder(&tx, root_id, &relative_path)?;
            folder_ids.insert(entry.path.clone(), folder_id);
        }
    }

    for entry in &result.entries {
        if let EntryKind::File {
            size,
            hash,
            modified,
        } = &entry.kind
        {
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
            let file = rfsync_storage::File {
                id: 0,
                folder_id,
                name: name.to_owned(),
                size: i64::try_from(*size)?,
                hash: hash.clone(),
                modified_secs: i64::try_from(modified.as_secs())?,
                modified_nanos: modified.subsec_nanos(),
            };

            rfsync_storage::insert_file(&tx, &file)?;
        }
    }
    rfsync_storage::mark_root_has_snapshot(&tx, root_id)?;
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
