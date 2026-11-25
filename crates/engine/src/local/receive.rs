use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
};

use super::path::resolve_in_root;
use crate::error::EngineError;

/// 받으려는 경로에 이미 파일이 있을 때의 처리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnExisting {
    /// 덮어쓰지 않고 실패한다. 내게 없는 파일을 받을 때 쓴다.
    Fail,
    /// 지금 있는 파일의 내용이 이 해시일 때만 덮어쓴다. 비교한 뒤 받는 동안 내가 고쳤다면 덮어쓰지 않고
    /// `EngineError::LocalChanged`로 실패한다(내 수정을 잃지 않기 위해). 파일이 없어졌어도 같다.
    ReplaceIfHash([u8; 32]),
}

/// 받는 도중의 파일 이름 앞에 붙이는 말. 끝나면 이름이 바뀌거나 지워진다.
const TEMP_PREFIX: &str = ".rfsync-tmp-";

const BUFFER_SIZE: usize = 64 * 1024;

/// 상대가 보낸 파일 내용을 내 루트 아래 `requested` 경로에 저장한다.
///
/// 1. 경로를 검증한다(글자 검증, 폴더가 심볼릭 링크로 루트 밖을 가리키지 않는지).
/// 2. 같은 폴더의 임시 파일에 `size`바이트를 받으면서 해시를 계산한다.
/// 3. 해시가 `expected_hash`와 맞을 때만 임시 파일의 이름을 바꿔 진짜 경로에 둔다.
///
/// 어느 단계에서든 실패하면 임시 파일은 지워지고 진짜 경로에는 아무것도 생기지 않는다.
/// 이미 같은 경로에 파일이 있을 때는 `on_existing`을 따른다. 덮어쓰는 경우에도 다 받은 뒤 이름 바꾸기 한 번으로
/// 바뀌므로 반쯤 쓴 파일이 보이지 않고, 이름을 바꾸기 직전에 지금 파일이 예상한 내용인지 한 번 더 확인한다.
/// (확인과 이름 바꾸기 사이의 아주 짧은 틈은 막지 못한다.)
///
/// 임시 파일은 이름을 바꾸는 일이 한 번에 끝나도록(같은 파일 시스템) 목적지와 같은 폴더에 만든다.
/// 그래서 받는 동안 루트 안에 `.rfsync-tmp-*` 파일이 보인다.
pub(crate) async fn save_received_file(
    root: &Path,
    requested: &str,
    size: u64,
    expected_hash: [u8; 32],
    on_existing: OnExisting,
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<(), EngineError> {
    let target = resolve_in_root(root, requested)?;
    let real_root = root.canonicalize()?;
    let parent = prepare_parent(&real_root, &target, requested)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| EngineError::InvalidEntryPath(PathBuf::from(requested)))?;
    let destination = parent.join(file_name);

    // 다 받고 나서야 실패하지 않도록 먼저 확인한다. 마지막 이름 바꾸기에서도 한 번 더 막는다.
    if on_existing == OnExisting::Fail && destination.symlink_metadata().is_ok() {
        return Err(already_exists(requested));
    }

    let (std_file, temp_path) = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(&parent)?
        .into_parts();
    let mut file = File::from_std(std_file);

    receive_into(&mut file, reader, size, expected_hash, requested).await?;
    drop(file);

    let persisted = match on_existing {
        OnExisting::Fail => temp_path.persist_noclobber(&destination),
        OnExisting::ReplaceIfHash(expected) => {
            // 받는 동안 내가 고쳤거나 지웠다면 덮어쓰지 않는다. 다음 비교가 새 상태로 다시 판단한다.
            if !has_content(&destination, expected).await? {
                return Err(EngineError::LocalChanged(requested.to_owned()));
            }
            temp_path.persist(&destination)
        }
    };
    persisted.map_err(|e| match e.error.kind() {
        ErrorKind::AlreadyExists => already_exists(requested),
        _ => EngineError::Io(e.error),
    })
}

/// `reader`에서 `size`바이트를 받아 `file`에 쓰고, 해시가 `expected_hash`와 맞는지 확인한 뒤 디스크에 내린다.
async fn receive_into(
    file: &mut File,
    reader: &mut (impl AsyncRead + Unpin),
    size: u64,
    expected_hash: [u8; 32],
    requested: &str,
) -> Result<(), EngineError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut remaining = size;
    while remaining > 0 {
        let want = remaining.min(BUFFER_SIZE as u64) as usize;
        let read = reader.read(&mut buffer[..want]).await?;
        if read == 0 {
            // 약속한 크기를 채우기 전에 스트림이 끝났다.
            return Err(io::Error::from(ErrorKind::UnexpectedEof).into());
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).await?;
        remaining -= read as u64;
    }

    if *hasher.finalize().as_bytes() != expected_hash {
        return Err(EngineError::HashMismatch(requested.to_owned()));
    }

    // 이름을 바꾸기 전에 내용을 디스크에 내려 두어, 중간에 꺼져도 반쯤 쓴 파일이 진짜 이름이 되지 않게 한다.
    file.flush().await?;
    file.sync_all().await?;
    Ok(())
}

/// 저장할 폴더가 루트 안에 있도록 확인하고, 없으면 만든다. 실제 경로(심볼릭 링크를 푼 것)를 돌려준다.
///
/// 폴더를 만들기 전에, 이미 있는 가장 가까운 조상이 루트 안인지 먼저 본다. 심볼릭 링크 폴더를 따라
/// 루트 밖에 폴더를 만들어 버리는 일을 막기 위해서다.
fn prepare_parent(
    real_root: &Path,
    target: &Path,
    requested: &str,
) -> Result<PathBuf, EngineError> {
    let outside = || EngineError::InvalidEntryPath(PathBuf::from(requested));
    let parent = target.parent().ok_or_else(outside)?;

    let existing = parent
        .ancestors()
        .find(|dir| dir.exists())
        .ok_or_else(outside)?;
    if !existing.canonicalize()?.starts_with(real_root) {
        return Err(outside());
    }

    std::fs::create_dir_all(parent)?;
    let real_parent = parent.canonicalize()?;
    if !real_parent.starts_with(real_root) {
        return Err(outside());
    }
    Ok(real_parent)
}

/// `path`가 일반 파일이고 내용의 해시가 `expected`인가. 없으면 거짓이다.
async fn has_content(path: &Path, expected: [u8; 32]) -> Result<bool, EngineError> {
    let path = path.to_path_buf();
    let hash = tokio::task::spawn_blocking(move || -> io::Result<Option<[u8; 32]>> {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !file.metadata()?.is_file() {
            return Ok(None);
        }
        let hash = blake3::Hasher::new().update_reader(&mut file)?.finalize();
        Ok(Some(*hash.as_bytes()))
    })
    .await
    .map_err(io::Error::other)??;
    Ok(hash == Some(expected))
}

fn already_exists(requested: &str) -> EngineError {
    EngineError::Io(io::Error::new(
        ErrorKind::AlreadyExists,
        format!("이미 있는 파일입니다: {requested}"),
    ))
}

#[cfg(test)]
mod tests;
