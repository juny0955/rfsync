use std::{
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::path::resolve_in_root;
use crate::error::EngineError;

/// `delete_if_unchanged`의 결과.
#[derive(Debug, PartialEq)]
pub(crate) enum DeleteOutcome {
    Deleted,
    /// 이미 없다. 할 일이 없다.
    AlreadyGone,
    /// 내용이 상대가 지운 파일과 다르다. 내가 그 사이에 고친 것이므로 지우지 않는다.
    Changed,
    /// 일반 파일이 아니다(폴더, 심볼릭 링크 등). 지우지 않는다.
    NotRegularFile,
}

/// 상대가 지웠다고 알린 파일을, 내 파일이 그 내용(`expected_hash`) 그대로일 때만 지운다.
///
/// 내용이 다르면 내가 고친 파일이므로 지우지 않는다(수정이 삭제를 이긴다). 삭제는 되돌릴 수 없어서
/// DB가 아니라 **디스크의 파일을 지금 읽어** 해시를 확인한다.
///
/// 경로는 `save_received_file`과 같은 수준으로 검증한다. 글자 검증 뒤에, 부모 폴더의 실제 경로(심볼릭 링크를 푼 것)가
/// 루트 안인지 확인한다. 파일 자체가 심볼릭 링크이면 링크가 가리키는 파일을 건드리지 않도록 지우지 않는다.
///
/// 해시를 확인한 뒤 지우기까지 아주 짧은 틈이 있어, 그 사이에 고친 내용은 막지 못한다.
pub(crate) fn delete_if_unchanged(
    root: &Path,
    requested: &str,
    expected_hash: &[u8; 32],
) -> Result<DeleteOutcome, EngineError> {
    let outside = || EngineError::InvalidEntryPath(PathBuf::from(requested));

    let target = resolve_in_root(root, requested)?;
    let real_root = root.canonicalize()?;
    let parent = target.parent().ok_or_else(outside)?;
    let real_parent = match parent.canonicalize() {
        Ok(real) => real,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(DeleteOutcome::AlreadyGone);
        }
        Err(e) => return Err(e.into()),
    };
    if !real_parent.starts_with(&real_root) {
        return Err(outside());
    }
    let destination = real_parent.join(target.file_name().ok_or_else(outside)?);

    let metadata = match destination.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(DeleteOutcome::AlreadyGone),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_file() {
        return Ok(DeleteOutcome::NotRegularFile);
    }

    let mut file = File::open(&destination)?;
    let hash = blake3::Hasher::new().update_reader(&mut file)?.finalize();
    drop(file);
    if hash.as_bytes() != expected_hash {
        return Ok(DeleteOutcome::Changed);
    }

    match std::fs::remove_file(&destination) {
        Ok(()) => Ok(DeleteOutcome::Deleted),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(DeleteOutcome::AlreadyGone),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests;
