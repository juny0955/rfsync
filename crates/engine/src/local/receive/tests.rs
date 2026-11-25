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
        OnExisting::Fail,
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
        OnExisting::Fail,
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
        OnExisting::Fail,
        &mut reader,
    )
    .await;

    assert!(matches!(result, Err(EngineError::Io(ref e)) if e.kind() == ErrorKind::UnexpectedEof));
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
        OnExisting::Fail,
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

    assert!(matches!(result, Err(EngineError::Io(ref e)) if e.kind() == ErrorKind::AlreadyExists));
    assert_eq!(
        fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "mine"
    );
    assert_eq!(leftover_temp_files(root.path()), 0);
}

#[tokio::test]
async fn 있는_파일이_예상한_내용이면_새_내용으로_바꾼다() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "mine").unwrap();
    let mut reader: &[u8] = b"theirs";

    save_received_file(
        root.path(),
        "a.txt",
        6,
        hash_of(b"theirs"),
        OnExisting::ReplaceIfHash(hash_of(b"mine")),
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
async fn 덮어쓰는_경우에도_해시가_틀리면_원래_파일을_그대로_둔다() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "mine").unwrap();
    let mut reader: &[u8] = b"theirs";

    let result = save_received_file(
        root.path(),
        "a.txt",
        6,
        hash_of(b"other"),
        OnExisting::ReplaceIfHash(hash_of(b"mine")),
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
async fn 받는_동안_내가_고친_파일은_덮어쓰지_않는다() {
    let root = tempfile::tempdir().unwrap();
    // 비교할 때는 "mine"이었지만 받는 동안 "edited"로 바뀐 상황이다.
    fs::write(root.path().join("a.txt"), "edited").unwrap();
    let mut reader: &[u8] = b"theirs";

    let result = save_received_file(
        root.path(),
        "a.txt",
        6,
        hash_of(b"theirs"),
        OnExisting::ReplaceIfHash(hash_of(b"mine")),
        &mut reader,
    )
    .await;

    assert!(matches!(result, Err(EngineError::LocalChanged(_))));
    assert_eq!(
        fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "edited"
    );
    assert_eq!(leftover_temp_files(root.path()), 0);
}

#[tokio::test]
async fn 받는_동안_내가_지운_파일은_되살리지_않는다() {
    let root = tempfile::tempdir().unwrap();
    let mut reader: &[u8] = b"theirs";

    let result = save_received_file(
        root.path(),
        "a.txt",
        6,
        hash_of(b"theirs"),
        OnExisting::ReplaceIfHash(hash_of(b"mine")),
        &mut reader,
    )
    .await;

    assert!(matches!(result, Err(EngineError::LocalChanged(_))));
    assert!(!root.path().join("a.txt").exists());
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
