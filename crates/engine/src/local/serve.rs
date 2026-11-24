use std::{
    fs::File,
    io::{ErrorKind, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use super::path::resolve_in_root;
use crate::error::EngineError;

/// 상대에게 보내려고 열어 둔 파일. 읽는 위치는 맨 앞이다.
pub(crate) struct ServedFile {
    pub file: File,
    /// 해시를 계산하며 실제로 읽은 바이트 수. 해시와 항상 짝이 맞는다.
    pub size: u64,
    pub hash: [u8; 32],
}

/// 상대가 요청한 경로의 파일을 열고 크기와 해시를 계산한다.
///
/// - 파일이 없거나 일반 파일이 아니면(폴더 등) `Ok(None)`이다.
/// - 경로 규칙을 어기거나 심볼릭 링크 등으로 루트 밖을 가리키면 `Err`이다.
///
/// 글자 검증(`resolve_in_root`) 뒤에 실제 경로(`canonicalize`)가 루트 안인지 한 번 더 확인한다.
/// 심볼릭 링크는 글자만 봐서는 알 수 없기 때문이다.
pub(crate) fn open_for_serving(
    root: &Path,
    requested: &str,
) -> Result<Option<ServedFile>, EngineError> {
    let path = resolve_in_root(root, requested)?;
    let real_root = root.canonicalize()?;

    let real_path = match path.canonicalize() {
        Ok(real) => real,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    if !real_path.starts_with(&real_root) {
        return Err(EngineError::InvalidEntryPath(PathBuf::from(requested)));
    }

    let mut file = File::open(&real_path)?;
    if !file.metadata()?.is_file() {
        return Ok(None);
    }

    let hash = *blake3::Hasher::new()
        .update_reader(&mut file)?
        .finalize()
        .as_bytes();
    // 파일 끝까지 읽었으므로 현재 위치가 곧 읽은 바이트 수다. 그다음 처음으로 되돌린다.
    let size = file.stream_position()?;
    file.seek(SeekFrom::Start(0))?;

    Ok(Some(ServedFile { file, size, hash }))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use tempfile::TempDir;

    use super::*;

    fn root_with(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        dir
    }

    #[test]
    fn 파일의_크기와_해시를_계산하고_읽기_위치는_맨_앞이다() {
        let root = root_with(&[("sub/a.txt", "hello")]);

        let mut served = open_for_serving(root.path(), "sub/a.txt").unwrap().unwrap();

        assert_eq!(served.size, 5);
        assert_eq!(served.hash, *blake3::hash(b"hello").as_bytes());
        let mut content = String::new();
        served.file.read_to_string(&mut content).unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn 빈_파일도_크기_0으로_연다() {
        let root = root_with(&[("empty.txt", "")]);

        let served = open_for_serving(root.path(), "empty.txt").unwrap().unwrap();

        assert_eq!(served.size, 0);
        assert_eq!(served.hash, *blake3::hash(b"").as_bytes());
    }

    #[test]
    fn 없는_파일과_폴더는_none이다() {
        let root = root_with(&[("sub/a.txt", "x"), ("plain.txt", "x")]);

        assert!(
            open_for_serving(root.path(), "missing.txt")
                .unwrap()
                .is_none()
        );
        assert!(open_for_serving(root.path(), "sub").unwrap().is_none());
        // 파일을 폴더처럼 쓴 경로
        assert!(
            open_for_serving(root.path(), "plain.txt/x")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn 경로_규칙을_어기면_에러다() {
        let root = root_with(&[("a.txt", "x")]);

        assert!(open_for_serving(root.path(), "../a.txt").is_err());
        assert!(open_for_serving(root.path(), "/etc/passwd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn 루트_밖을_가리키는_심볼릭_링크는_에러다() {
        let outside = root_with(&[("secret.txt", "secret")]);
        let root = root_with(&[("a.txt", "x")]);
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linkdir")).unwrap();

        assert!(open_for_serving(root.path(), "link.txt").is_err());
        assert!(open_for_serving(root.path(), "linkdir/secret.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn 루트_안을_가리키는_심볼릭_링크는_연다() {
        let root = root_with(&[("real.txt", "hello")]);
        std::os::unix::fs::symlink(root.path().join("real.txt"), root.path().join("link.txt"))
            .unwrap();

        let served = open_for_serving(root.path(), "link.txt").unwrap().unwrap();

        assert_eq!(served.size, 5);
    }
}
