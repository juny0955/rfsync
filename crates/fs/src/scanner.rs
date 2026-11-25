use std::fs::File;
use std::fs::{self, symlink_metadata};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::error::FsError;

#[derive(Debug)]
pub struct FsEntry {
    pub path: PathBuf,
    pub kind: FsEntryKind,
}

impl FsEntry {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, FsEntryKind::Directory)
    }
}

#[derive(Debug)]
pub enum FsEntryKind {
    File {
        size: u64,
        hash: String,
        modified: SystemTime,
    },
    Directory,
}

/// `root` 아래의 경로를 `root` 기준 상대 경로로 반환합니다. 심볼릭 링크는 따라가지 않습니다.
///
/// # Errors
/// 디렉터리를 읽거나 항목을 확인할 수 없으면 오류를 반환합니다.
pub fn scan(root: &Path) -> Result<Vec<FsEntry>, FsError> {
    let mut directories = vec![PathBuf::new()];
    let mut entries = Vec::new();

    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(root.join(&directory))? {
            let entry = entry?;
            let path = directory.join(entry.file_name());

            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(path.clone());
                entries.push(FsEntry {
                    path,
                    kind: FsEntryKind::Directory,
                });
            } else if file_type.is_file() {
                entries.push(FsEntry {
                    path,
                    kind: scan_file(&entry.path())?,
                });
            }
        }
    }

    Ok(entries)
}

pub fn scan_path(root: &Path, path: &Path) -> Result<Option<FsEntry>, FsError> {
    if path.as_os_str().is_empty()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(FsError::InvalidRelativePath(path.to_path_buf()));
    }

    let mut full_path = root.to_path_buf();
    let mut components = path.components().peekable();

    while let Some(component) = components.next() {
        full_path.push(component.as_os_str());

        let metadata = match symlink_metadata(&full_path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            return Ok(None);
        }

        // 중간 경로인데 현재 component가 dir 인지 검사
        if components.peek().is_some() {
            if !file_type.is_dir() {
                return Ok(None);
            }
            continue;
        }

        let kind = if file_type.is_dir() {
            FsEntryKind::Directory
        } else if file_type.is_file() {
            scan_file(&full_path)?
        } else {
            return Ok(None);
        };

        return Ok(Some(FsEntry {
            path: path.to_path_buf(),
            kind,
        }));
    }

    unreachable!("unreachable")
}

fn hash_file(file: &File) -> Result<String, FsError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn scan_file(path: &Path) -> Result<FsEntryKind, FsError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;

    Ok(FsEntryKind::File {
        size: metadata.len(),
        hash: hash_file(&file)?,
        modified: metadata.modified()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> Result<PathBuf, FsError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        loop {
            let root = std::env::temp_dir().join(format!(
                "rfsync-scan-path-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Ok(root),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[test]
    fn 동시에_만든_테스트_루트는_서로_다르다() -> Result<(), FsError> {
        let barrier = std::sync::Barrier::new(16);
        let roots = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        test_root()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Result<Vec<_>, _>>()
        })?;

        assert_eq!(
            roots.iter().collect::<std::collections::HashSet<_>>().len(),
            roots.len()
        );
        for root in roots {
            fs::remove_dir_all(root)?;
        }
        Ok(())
    }

    #[test]
    fn scan_path는_파일의_메타데이터와_해시를_읽는다() -> Result<(), FsError> {
        let root = test_root()?;
        fs::create_dir(root.join("nested"))?;
        fs::write(root.join("nested/file"), b"test")?;
        let modified = fs::metadata(root.join("nested/file"))?.modified()?;

        let entry = scan_path(&root, Path::new("nested/file"))?.unwrap();

        assert_eq!(entry.path, Path::new("nested/file"));
        assert!(
            matches!(entry.kind, FsEntryKind::File { size: 4, ref hash, modified: time }
            if hash == &blake3::hash(b"test").to_hex().to_string() && time == modified)
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn scan_path는_디렉터리의_자식을_스캔하지_않고_반환한다() -> Result<(), FsError> {
        let root = test_root()?;
        fs::create_dir(root.join("nested"))?;
        fs::write(root.join("nested/file"), b"test")?;

        let entry = scan_path(&root, Path::new("nested"))?.unwrap();

        assert_eq!(entry.path, Path::new("nested"));
        assert!(matches!(entry.kind, FsEntryKind::Directory));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn scan_path는_없는_경로나_파일인_부모에는_none을_반환한다() -> Result<(), FsError> {
        let root = test_root()?;
        fs::write(root.join("file"), b"test")?;

        assert!(scan_path(&root, Path::new("missing/file"))?.is_none());
        assert!(scan_path(&root, Path::new("file/child"))?.is_none());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn scan_path는_잘못된_상대_경로를_거부한다() {
        for path in [Path::new(""), Path::new("../file"), Path::new("/file")] {
            assert!(matches!(scan_path(Path::new("unused"), path),
                Err(FsError::InvalidRelativePath(invalid)) if invalid == path));
        }
    }

    #[cfg(unix)]
    #[test]
    fn scan_path는_마지막과_중간의_심볼릭_링크를_제외한다() -> Result<(), FsError> {
        let root = test_root()?;
        fs::create_dir(root.join("nested"))?;
        fs::write(root.join("nested/file"), b"test")?;
        std::os::unix::fs::symlink(root.join("nested"), root.join("link"))?;
        std::os::unix::fs::symlink(root.join("nested/file"), root.join("file-link"))?;

        assert!(scan_path(&root, Path::new("link/file"))?.is_none());
        assert!(scan_path(&root, Path::new("file-link"))?.is_none());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn 중첩된_파일과_빈_디렉터리를_스캔한다() -> Result<(), FsError> {
        let name = format!(
            "rfsync-scan-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(name);
        fs::create_dir(&root)?;
        fs::create_dir(root.join("nested"))?;
        fs::create_dir(root.join("nested/empty"))?;
        fs::write(root.join("nested/file.txt"), b"test")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("nested/link"))?;

        let modified = fs::metadata(root.join("nested/file.txt"))?.modified()?;
        let entries = scan(&root)?;
        fs::remove_dir_all(&root)?;

        let mut paths: Vec<_> = entries.iter().map(|entry| entry.path.clone()).collect();
        paths.sort();
        let expected = vec![
            PathBuf::from("nested"),
            PathBuf::from("nested/empty"),
            PathBuf::from("nested/file.txt"),
        ];
        assert_eq!(paths, expected);

        for entry in &entries {
            if entry.path == Path::new("nested/file.txt") {
                assert!(
                    matches!(entry.kind, FsEntryKind::File { size: 4, modified: time, .. } if time == modified)
                );
            } else {
                assert!(matches!(entry.kind, FsEntryKind::Directory));
            }
        }

        Ok(())
    }

    #[test]
    fn 파일_내용을_해시한다() -> Result<(), FsError> {
        let root = std::env::temp_dir().join(format!(
            "rfsync-hash-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root)?;
        fs::write(root.join("empty"), b"")?;
        fs::write(root.join("first"), b"same contents")?;
        fs::write(root.join("second"), b"same contents")?;
        fs::write(root.join("different"), b"different contents")?;

        let result = scan(&root)?;
        fs::remove_dir_all(&root)?;

        let hash = |name| {
            let entry = result
                .iter()
                .find(|entry| entry.path == Path::new(name))
                .unwrap();
            match &entry.kind {
                FsEntryKind::File { hash, .. } => hash.as_str(),
                FsEntryKind::Directory => panic!("expected a file"),
            }
        };
        assert_eq!(
            hash("empty"),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(hash("first"), hash("second"));
        assert_ne!(hash("first"), hash("different"));
        Ok(())
    }

    #[test]
    fn 잘못된_루트는_에러를_반환한다() {
        assert!(matches!(
            scan(Path::new("\0")),
            Err(FsError::Scan(error)) if error.kind() == std::io::ErrorKind::InvalidInput
        ));
    }
}
