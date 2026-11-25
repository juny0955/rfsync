use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_protocol::VersionVector;

use crate::error::EngineError;

/// UNIX 스타일로 경로 변환
/// error: UTF-8 표현 불가 및 '\' 포함
pub(super) fn portable_path(path: &Path) -> Result<String, EngineError> {
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

pub(super) fn to_storage_file(
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
        deleted: false,
        // 감시자가 내 카운터를 올리기 전까지는 빈 벡터다. 빈 바이트가 아니라 직렬화한 빈 벡터를 넣는다.
        version: VersionVector::default().to_bytes()?,
    })
}
