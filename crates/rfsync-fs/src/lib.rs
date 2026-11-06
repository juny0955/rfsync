use std::fs;
use std::fs::File;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug)]
pub struct ScanResult {
    pub entries: Vec<Entry>,
    pub skipped: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

#[derive(Debug)]
pub enum EntryKind {
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
pub fn scan(root: &Path) -> io::Result<ScanResult> {
    let mut directories = vec![PathBuf::new()];
    let mut entries = Vec::new();
    let mut skipped = Vec::new();

    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(root.join(&directory))? {
            let entry = entry?;
            let path = directory.join(entry.file_name());

            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(path.clone());
                entries.push(Entry {
                    path,
                    kind: EntryKind::Directory,
                });
            } else if file_type.is_file() {
                let file = File::open(entry.path())?;
                let metadata = file.metadata()?;
                let hash = hash_file(&file)?;

                entries.push(Entry {
                    path,
                    kind: EntryKind::File {
                        hash,
                        size: metadata.len(),
                        modified: metadata.modified()?,
                    },
                });
            } else {
                skipped.push(path);
            }
        }
    }

    Ok(ScanResult { entries, skipped })
}

fn hash_file(file: &File) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file)?;
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scans_nested_files_and_empty_directories() -> io::Result<()> {
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
        let result = scan(&root)?;
        fs::remove_dir_all(&root)?;

        let mut paths: Vec<_> = result
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        paths.sort();
        let expected = vec![
            PathBuf::from("nested"),
            PathBuf::from("nested/empty"),
            PathBuf::from("nested/file.txt"),
        ];
        assert_eq!(paths, expected);

        for entry in &result.entries {
            if entry.path == Path::new("nested/file.txt") {
                assert!(
                    matches!(entry.kind, EntryKind::File { size: 4, modified: time, .. } if time == modified)
                );
            } else {
                assert!(matches!(entry.kind, EntryKind::Directory));
            }
        }

        #[cfg(unix)]
        assert_eq!(result.skipped, vec![PathBuf::from("nested/link")]);
        #[cfg(not(unix))]
        assert!(result.skipped.is_empty());
        Ok(())
    }

    #[test]
    fn hashes_file_contents() -> io::Result<()> {
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
                .entries
                .iter()
                .find(|entry| entry.path == Path::new(name))
                .unwrap();
            match &entry.kind {
                EntryKind::File { hash, .. } => hash.as_str(),
                EntryKind::Directory => panic!("expected a file"),
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
    fn returns_error_for_invalid_root() {
        assert_eq!(
            scan(Path::new("\0")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
