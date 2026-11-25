use std::fs;

use tempfile::TempDir;

use super::*;

fn hash_of(content: &[u8]) -> [u8; 32] {
    *blake3::hash(content).as_bytes()
}

fn root_with(files: &[(&str, &str)]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let full = root.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    root
}

#[test]
fn 파일을_같은_폴더의_다른_이름으로_남긴다() {
    let root = root_with(&[("sub/a.txt", "mine")]);

    let outcome = preserve_file(root.path(), "sub/a.txt", "sub/a.conflict.txt").unwrap();

    assert_eq!(outcome, PreserveOutcome::Preserved(hash_of(b"mine")));
    assert_eq!(fs::read(root.path().join("sub/a.txt")).unwrap(), b"mine");
    assert_eq!(
        fs::read(root.path().join("sub/a.conflict.txt")).unwrap(),
        b"mine"
    );
}

#[test]
fn 원래_이름이_새_파일로_바뀌어도_보존한_내용은_그대로다() {
    let root = root_with(&[("a.txt", "mine")]);
    preserve_file(root.path(), "a.txt", "a.conflict.txt").unwrap();

    // 받은 파일이 임시 파일의 이름 바꾸기로 원래 이름을 덮어쓰는 것과 같다.
    fs::write(root.path().join("incoming.tmp"), "theirs").unwrap();
    fs::rename(root.path().join("incoming.tmp"), root.path().join("a.txt")).unwrap();

    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"theirs");
    assert_eq!(
        fs::read(root.path().join("a.conflict.txt")).unwrap(),
        b"mine"
    );
}

#[test]
fn 보존할_이름에_내용이_같은_파일이_이미_있으면_그대로_쓴다() {
    let root = root_with(&[("a.txt", "mine"), ("a.conflict.txt", "mine")]);

    let outcome = preserve_file(root.path(), "a.txt", "a.conflict.txt").unwrap();

    assert_eq!(outcome, PreserveOutcome::Preserved(hash_of(b"mine")));
}

#[test]
fn 보존할_이름에_다른_내용이_이미_있으면_덮어쓰지_않는다() {
    let root = root_with(&[("a.txt", "mine"), ("a.conflict.txt", "older")]);

    let outcome = preserve_file(root.path(), "a.txt", "a.conflict.txt").unwrap();

    assert_eq!(outcome, PreserveOutcome::AlreadyExists);
    assert_eq!(
        fs::read(root.path().join("a.conflict.txt")).unwrap(),
        b"older"
    );
}

#[test]
fn 파일이_없거나_폴더면_남기지_않는다() {
    let root = root_with(&[("dir/inner.txt", "x")]);

    assert_eq!(
        preserve_file(root.path(), "gone.txt", "gone.conflict.txt").unwrap(),
        PreserveOutcome::SourceMissing
    );
    assert_eq!(
        preserve_file(root.path(), "no/such/gone.txt", "no/such/gone.conflict.txt").unwrap(),
        PreserveOutcome::SourceMissing
    );
    assert_eq!(
        preserve_file(root.path(), "dir", "dir.conflict").unwrap(),
        PreserveOutcome::NotRegularFile
    );
}

#[test]
fn 복사는_새_파일에만_하고_이미_있으면_건드리지_않는다() {
    let root = root_with(&[("a.txt", "mine"), ("taken.txt", "other")]);

    copy_exclusive(&root.path().join("a.txt"), &root.path().join("copy.txt")).unwrap();
    let taken =
        copy_exclusive(&root.path().join("a.txt"), &root.path().join("taken.txt")).unwrap_err();

    assert_eq!(fs::read(root.path().join("copy.txt")).unwrap(), b"mine");
    assert_eq!(taken.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read(root.path().join("taken.txt")).unwrap(), b"other");
}

#[cfg(unix)]
#[test]
fn 복사_대상이_심볼릭_링크여도_링크가_가리키는_파일을_건드리지_않는다() {
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.txt");
    fs::write(&victim, "original").unwrap();
    let root = root_with(&[("a.txt", "mine")]);
    std::os::unix::fs::symlink(&victim, root.path().join("link.txt")).unwrap();

    let result = copy_exclusive(&root.path().join("a.txt"), &root.path().join("link.txt"));

    assert_eq!(result.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&victim).unwrap(), b"original");
}

#[test]
fn 경로_규칙을_어기거나_폴더가_다르면_실패한다() {
    let root = root_with(&[("a.txt", "x"), ("sub/b.txt", "y")]);

    assert!(preserve_file(root.path(), "../a.txt", "../a.conflict.txt").is_err());
    assert!(preserve_file(root.path(), "a.txt", "sub/a.conflict.txt").is_err());
}

#[cfg(unix)]
#[test]
fn 심볼릭_링크_파일은_남기지_않는다() {
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.txt");
    fs::write(&victim, "original").unwrap();
    let root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&victim, root.path().join("a.txt")).unwrap();

    let outcome = preserve_file(root.path(), "a.txt", "a.conflict.txt").unwrap();

    assert_eq!(outcome, PreserveOutcome::NotRegularFile);
    assert!(!root.path().join("a.conflict.txt").exists());
}
