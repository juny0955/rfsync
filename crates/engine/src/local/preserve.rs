use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::path::resolve_in_root;
use crate::error::EngineError;

/// `preserve_file`의 결과.
#[derive(Debug, PartialEq)]
pub(crate) enum PreserveOutcome {
    /// 남겼다. 값은 남긴 시점 내용의 해시다. 이어서 이 파일을 새 내용으로 바꿀 때 그 사이에 또 바뀌었는지 확인하는 데 쓴다.
    Preserved([u8; 32]),
    /// 보존할 파일이 이미 없다. 지킬 내용이 없으니 할 일이 없다.
    SourceMissing,
    /// 일반 파일이 아니다(폴더, 심볼릭 링크 등). 보존하지 않는다.
    NotRegularFile,
    /// 보존할 이름에 다른 내용의 파일이 이미 있다. 덮어쓰지 않는다.
    AlreadyExists,
}

/// 충돌에서 질 내 파일(`requested`)을 같은 폴더의 `preserved` 이름으로 남겨 둔다.
///
/// 두 이름이 같은 폴더에 있어야 한다. 하드 링크로 만들어서 같은 순간의 내용이 그대로 남고,
/// 이어서 원래 이름이 새 파일로 바뀌어도(이름 바꾸기로 덮어쓰기) 보존한 쪽은 옛 내용을 유지한다.
/// 하드 링크를 지원하지 않는 곳에서는 내용을 복사한다(`copy_exclusive`).
///
/// 하드 링크는 독립된 복사본이 아니다. 링크를 만든 뒤 누가 원본을 제자리에서 고치거나 줄이면(에디터의 교체 저장은
/// 새 파일을 만들어 해당하지 않는다) 남긴 파일에도 그대로 반영된다. 그래도 내용이 사라지지는 않고 그 최신 내용이 남는다.
/// 복사본으로 만들면 원본이 쓰이는 중에 읽어 내용이 섞일 수 있어 하드 링크를 택했다.
/// 반환값의 해시는 남긴 시점 내용이므로, 호출한 쪽이 그 뒤 바뀌었는지 확인할 수 있다.
///
/// 보존할 이름에 내용이 같은 파일이 이미 있으면 이전 시도에서 남긴 것이므로 그대로 쓰고 `Preserved`다.
/// 받기가 실패해 같은 충돌을 다시 처리할 때 멈추지 않게 하기 위해서다. 내용이 다르면 `AlreadyExists`다.
///
/// 경로는 `save_received_file`과 같은 수준으로 검증한다. 글자 검증 뒤에, 부모 폴더의 실제 경로(심볼릭 링크를 푼 것)가
/// 루트 안인지 확인하고, 파일 자체가 심볼릭 링크이면 링크가 가리키는 파일을 건드리지 않도록 보존하지 않는다.
pub(crate) fn preserve_file(
    root: &Path,
    requested: &str,
    preserved: &str,
) -> Result<PreserveOutcome, EngineError> {
    let outside = || EngineError::InvalidEntryPath(PathBuf::from(requested));

    let source = resolve_in_root(root, requested)?;
    let target = resolve_in_root(root, preserved)?;
    if source.parent() != target.parent() {
        return Err(outside());
    }

    let real_root = root.canonicalize()?;
    let parent = source.parent().ok_or_else(outside)?;
    let real_parent = match parent.canonicalize() {
        Ok(real) => real,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(PreserveOutcome::SourceMissing);
        }
        Err(e) => return Err(e.into()),
    };
    if !real_parent.starts_with(&real_root) {
        return Err(outside());
    }
    let source = real_parent.join(source.file_name().ok_or_else(outside)?);
    let target = real_parent.join(target.file_name().ok_or_else(outside)?);

    let metadata = match source.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(PreserveOutcome::SourceMissing),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_file() {
        return Ok(PreserveOutcome::NotRegularFile);
    }
    let hash = match hash_file(&source) {
        Ok(hash) => hash,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(PreserveOutcome::SourceMissing),
        Err(e) => return Err(e.into()),
    };
    if let Ok(existing) = target.symlink_metadata() {
        let same = existing.is_file() && hash_file(&target)? == hash;
        return Ok(if same {
            PreserveOutcome::Preserved(hash)
        } else {
            PreserveOutcome::AlreadyExists
        });
    }

    match std::fs::hard_link(&source, &target) {
        Ok(()) => Ok(PreserveOutcome::Preserved(hash)),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(PreserveOutcome::AlreadyExists),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(PreserveOutcome::SourceMissing),
        Err(_) => match copy_exclusive(&source, &target) {
            Ok(()) => Ok(PreserveOutcome::Preserved(hash)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(PreserveOutcome::AlreadyExists),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PreserveOutcome::SourceMissing),
            Err(e) => Err(e.into()),
        },
    }
}

/// `source`의 내용을 새 파일 `target`로 복사한다. `target`이 이미 있으면(다른 작업이 그 사이에 만든 파일이나
/// 심볼릭 링크 포함) 건드리지 않고 `AlreadyExists`다. 복사 중 실패하면 반쯤 쓴 `target`을 지운다.
fn copy_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut from = std::fs::File::open(source)?;
    // `create_new`는 존재 확인과 생성이 한 번에 일어나고, 있는 심볼릭 링크를 따라가지 않는다.
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let copied = std::io::copy(&mut from, &mut to).and_then(|_| to.sync_all());
    if copied.is_err() {
        drop(to);
        let _ = std::fs::remove_file(target);
    }
    copied
}

/// 파일 내용의 해시.
fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let hash = blake3::Hasher::new()
        .update_reader(&mut std::fs::File::open(path)?)?
        .finalize();
    Ok(*hash.as_bytes())
}

#[cfg(test)]
mod tests;
