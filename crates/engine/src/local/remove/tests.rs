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
fn 내용이_같으면_파일을_지운다() {
    let root = root_with(&[("sub/a.txt", "hello")]);

    let outcome = delete_if_unchanged(root.path(), "sub/a.txt", &hash_of(b"hello")).unwrap();

    assert_eq!(outcome, DeleteOutcome::Deleted);
    assert!(!root.path().join("sub/a.txt").exists());
}

#[test]
fn 내용이_다르면_내가_고친_파일이므로_지우지_않는다() {
    let root = root_with(&[("a.txt", "edited by me")]);

    let outcome = delete_if_unchanged(root.path(), "a.txt", &hash_of(b"old")).unwrap();

    assert_eq!(outcome, DeleteOutcome::Changed);
    assert_eq!(
        fs::read(root.path().join("a.txt")).unwrap(),
        b"edited by me"
    );
}

#[test]
fn 이미_없으면_할_일이_없다() {
    let root = root_with(&[]);

    for requested in ["gone.txt", "no/such/dir/gone.txt"] {
        let outcome = delete_if_unchanged(root.path(), requested, &hash_of(b"x")).unwrap();
        assert_eq!(outcome, DeleteOutcome::AlreadyGone, "{requested}");
    }
}

#[test]
fn 폴더는_지우지_않는다() {
    let root = root_with(&[("dir/inner.txt", "x")]);

    let outcome = delete_if_unchanged(root.path(), "dir", &hash_of(b"x")).unwrap();

    assert_eq!(outcome, DeleteOutcome::NotRegularFile);
    assert!(root.path().join("dir/inner.txt").exists());
}

#[test]
fn 경로_규칙을_어기면_에러이고_아무것도_지우지_않는다() {
    let root = root_with(&[("a.txt", "hello")]);
    let outside = root_with(&[("secret.txt", "hello")]);
    let escape = format!(
        "../{}/secret.txt",
        outside.path().file_name().unwrap().to_string_lossy()
    );

    for requested in ["", "..", "../a.txt", "/etc/passwd", "a\\b", escape.as_str()] {
        assert!(
            delete_if_unchanged(root.path(), requested, &hash_of(b"hello")).is_err(),
            "{requested:?}가 통과했다"
        );
    }
    assert!(root.path().join("a.txt").exists());
    assert!(outside.path().join("secret.txt").exists());
}

#[cfg(unix)]
#[test]
fn 루트_밖을_가리키는_심볼릭_링크_폴더_안의_파일은_지우지_않는다() {
    let root = root_with(&[]);
    let outside = root_with(&[("victim.txt", "hello")]);
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();

    let result = delete_if_unchanged(root.path(), "link/victim.txt", &hash_of(b"hello"));

    assert!(matches!(result, Err(EngineError::InvalidEntryPath(_))));
    assert!(outside.path().join("victim.txt").exists());
}

#[cfg(unix)]
#[test]
fn 파일이_심볼릭_링크이면_링크가_가리키는_파일을_지우지_않는다() {
    let root = root_with(&[]);
    let outside = root_with(&[("target.txt", "hello")]);
    std::os::unix::fs::symlink(outside.path().join("target.txt"), root.path().join("a.txt"))
        .unwrap();

    let outcome = delete_if_unchanged(root.path(), "a.txt", &hash_of(b"hello")).unwrap();

    assert_eq!(outcome, DeleteOutcome::NotRegularFile);
    assert!(outside.path().join("target.txt").exists());
}
