use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use rfsync_fs::{FsEntry, FsEntryKind};
use rfsync_protocol::DeviceId;
use rfsync_storage::SyncPending;

use super::version::next_file_version;
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
    version: Vec<u8>,
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
        version,
    })
}

/// 디스크에서 본 변경을 저장할 행으로 바꾼다. version은 이전 행(`previous`), 새 해시, 상대에게서 받은 기록(`pending`)으로 정한다.
pub(super) fn to_local_storage_file(
    entry: &FsEntry,
    id: i64,
    folder_ids: &HashMap<PathBuf, i64>,
    previous: Option<&rfsync_storage::File>,
    pending: Option<&SyncPending>,
    me: DeviceId,
) -> Result<rfsync_storage::File, EngineError> {
    let mut file = to_storage_file(entry, id, folder_ids, Vec::new())?;
    file.version = next_file_version(previous, &file.hash, pending, me)?;
    Ok(file)
}
