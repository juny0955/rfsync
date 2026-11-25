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
mod tests {
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
}
