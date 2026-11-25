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
/// `overwrite`가 거짓이면 이미 같은 경로에 파일이 있을 때 덮어쓰지 않고 실패한다(내게 없는 파일을 받을 때).
/// 참이면 이미 있는 파일을 새 내용으로 바꾼다(상대가 더 최신인 파일을 받을 때). 이름 바꾸기 한 번으로
/// 바뀌므로 덮어쓰는 순간에도 반쯤 쓴 파일이 보이지 않는다.
///
/// 임시 파일은 이름을 바꾸는 일이 한 번에 끝나도록(같은 파일 시스템) 목적지와 같은 폴더에 만든다.
/// 그래서 받는 동안 루트 안에 `.rfsync-tmp-*` 파일이 보인다.
pub(crate) async fn save_received_file(
    root: &Path,
    requested: &str,
    size: u64,
    expected_hash: [u8; 32],
    overwrite: bool,
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
    if !overwrite && destination.symlink_metadata().is_ok() {
        return Err(already_exists(requested));
    }

    let (std_file, temp_path) = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(&parent)?
        .into_parts();
    let mut file = File::from_std(std_file);

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
    drop(file);

    let persisted = if overwrite {
        temp_path.persist(&destination)
    } else {
        temp_path.persist_noclobber(&destination)
    };
    persisted.map_err(|e| match e.error.kind() {
        ErrorKind::AlreadyExists => already_exists(requested),
        _ => EngineError::Io(e.error),
    })
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

fn already_exists(requested: &str) -> EngineError {
    EngineError::Io(io::Error::new(
        ErrorKind::AlreadyExists,
        format!("이미 있는 파일입니다: {requested}"),
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// `root` 아래에 남은 `.rfsync-tmp-*` 파일의 개수.
    fn leftover_temp_files(root: &Path) -> usize {
        fn walk(dir: &Path, count: &mut usize) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, count);
                } else if path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(TEMP_PREFIX)
                {
                    *count += 1;
                }
            }
        }
        let mut count = 0;
        walk(root, &mut count);
        count
    }

    fn hash_of(content: &[u8]) -> [u8; 32] {
        *blake3::hash(content).as_bytes()
    }

    async fn save(root: &TempDir, requested: &str, content: &[u8]) -> Result<(), EngineError> {
        let mut reader = content;
        save_received_file(
            root.path(),
            requested,
            content.len() as u64,
            hash_of(content),
            false,
            &mut reader,
        )
        .await
    }

    #[tokio::test]
    async fn 받은_내용을_루트_아래_경로에_저장하고_폴더도_만든다() {
        let root = tempfile::tempdir().unwrap();

        save(&root, "sub/deep/a.txt", b"hello").await.unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join("sub/deep/a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn 빈_파일도_저장한다() {
        let root = tempfile::tempdir().unwrap();

        save(&root, "empty.txt", b"").await.unwrap();

        assert_eq!(fs::read(root.path().join("empty.txt")).unwrap(), b"");
    }

    #[tokio::test]
    async fn 버퍼보다_큰_파일도_순서대로_저장한다() {
        let root = tempfile::tempdir().unwrap();
        // 버퍼(64 KiB)의 정수배가 아닌 크기. 바이트마다 값이 달라야 순서가 틀리면 드러난다.
        let content: Vec<u8> = (0..3 * 1024 * 1024 + 123)
            .map(|i| (i % 251) as u8)
            .collect();

        save(&root, "big.bin", &content).await.unwrap();

        assert!(fs::read(root.path().join("big.bin")).unwrap() == content);
    }

    #[tokio::test]
    async fn 해시가_맞지_않으면_실패하고_아무것도_남지_않는다() {
        let root = tempfile::tempdir().unwrap();
        let mut reader: &[u8] = b"hello";

        let result = save_received_file(
            root.path(),
            "a.txt",
            5,
            hash_of(b"other"),
            false,
            &mut reader,
        )
        .await;

        assert!(matches!(result, Err(EngineError::HashMismatch(_))));
        assert!(!root.path().join("a.txt").exists());
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn 약속한_크기보다_일찍_끝나면_실패하고_아무것도_남지_않는다() {
        let root = tempfile::tempdir().unwrap();
        let mut reader: &[u8] = b"hello";

        let result = save_received_file(
            root.path(),
            "a.txt",
            10,
            hash_of(b"hello"),
            false,
            &mut reader,
        )
        .await;

        assert!(
            matches!(result, Err(EngineError::Io(ref e)) if e.kind() == ErrorKind::UnexpectedEof)
        );
        assert!(!root.path().join("a.txt").exists());
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn 약속한_크기보다_많이_와도_크기만큼만_읽는다() {
        let root = tempfile::tempdir().unwrap();
        let mut reader: &[u8] = b"helloEXTRA";

        save_received_file(
            root.path(),
            "a.txt",
            5,
            hash_of(b"hello"),
            false,
            &mut reader,
        )
        .await
        .unwrap();

        assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"hello");
        assert_eq!(reader, b"EXTRA", "남은 바이트는 읽지 않고 그대로 둔다");
    }

    #[tokio::test]
    async fn 이미_있는_파일은_덮어쓰지_않는다() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "mine").unwrap();

        let result = save(&root, "a.txt", b"theirs").await;

        assert!(
            matches!(result, Err(EngineError::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists)
        );
        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "mine"
        );
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn overwrite가_참이면_이미_있는_파일을_새_내용으로_바꾼다() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "mine").unwrap();
        let mut reader: &[u8] = b"theirs";

        save_received_file(
            root.path(),
            "a.txt",
            6,
            hash_of(b"theirs"),
            true,
            &mut reader,
        )
        .await
        .unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "theirs"
        );
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn overwrite여도_해시가_틀리면_원래_파일을_그대로_둔다() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "mine").unwrap();
        let mut reader: &[u8] = b"theirs";

        let result = save_received_file(
            root.path(),
            "a.txt",
            6,
            hash_of(b"other"),
            true,
            &mut reader,
        )
        .await;

        assert!(matches!(result, Err(EngineError::HashMismatch(_))));
        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "mine"
        );
        assert_eq!(leftover_temp_files(root.path()), 0);
    }

    #[tokio::test]
    async fn 경로_규칙을_어기면_실패하고_아무것도_만들지_않는다() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().parent().unwrap().to_owned();

        assert!(save(&root, "../escaped.txt", b"x").await.is_err());
        assert!(save(&root, "/tmp/escaped.txt", b"x").await.is_err());

        assert!(!outer.join("escaped.txt").exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 루트_밖을_가리키는_심볼릭_링크_폴더에는_저장하지_않는다() {
        let outside = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linkdir")).unwrap();

        // 링크 폴더 바로 아래, 그리고 링크 폴더 밑에 새 폴더를 만들어야 하는 경우 둘 다
        assert!(save(&root, "linkdir/a.txt", b"x").await.is_err());
        assert!(save(&root, "linkdir/new/a.txt", b"x").await.is_err());

        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 목적지가_심볼릭_링크여도_링크가_가리키는_파일을_건드리지_않는다() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        fs::write(&victim, "original").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&victim, root.path().join("a.txt")).unwrap();

        let result = save(&root, "a.txt", b"theirs").await;

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "original");
    }
}
