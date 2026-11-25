use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::path::resolve_in_root;
use crate::error::EngineError;

/// `preserve_file`의 결과.
#[derive(Debug, PartialEq)]
pub(crate) enum PreserveOutcome {
    Preserved,
    /// 보존할 파일이 이미 없다. 지킬 내용이 없으니 할 일이 없다.
    SourceMissing,
    /// 일반 파일이 아니다(폴더, 심볼릭 링크 등). 보존하지 않는다.
    NotRegularFile,
    /// 보존할 이름이 이미 있다. 덮어쓰지 않는다.
    AlreadyExists,
}

/// 충돌에서 질 내 파일(`requested`)을 같은 폴더의 `preserved` 이름으로 남겨 둔다.
///
/// 두 이름이 같은 폴더에 있어야 한다. 하드 링크로 만들어서 같은 순간의 내용이 그대로 남고,
/// 이어서 원래 이름이 새 파일로 바뀌어도(이름 바꾸기로 덮어쓰기) 보존한 쪽은 옛 내용을 유지한다.
/// 하드 링크를 지원하지 않는 곳에서는 내용을 복사한다.
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
    if target.symlink_metadata().is_ok() {
        return Ok(PreserveOutcome::AlreadyExists);
    }

    match std::fs::hard_link(&source, &target) {
        Ok(()) => Ok(PreserveOutcome::Preserved),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(PreserveOutcome::AlreadyExists),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(PreserveOutcome::SourceMissing),
        Err(_) => match std::fs::copy(&source, &target) {
            Ok(_) => Ok(PreserveOutcome::Preserved),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PreserveOutcome::SourceMissing),
            Err(e) => Err(e.into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

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

        assert_eq!(outcome, PreserveOutcome::Preserved);
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
    fn 보존할_이름이_이미_있으면_덮어쓰지_않는다() {
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
}
