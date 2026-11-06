use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `root` 아래의 경로를 `root` 기준 상대 경로로 반환합니다. 심볼릭 링크는 따라가지 않습니다.
///
/// # Errors
/// 디렉터리를 읽거나 항목을 확인할 수 없으면 오류를 반환합니다.
pub fn scan(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut directories = vec![PathBuf::new()];

    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(root.join(&directory))? {
            let entry = entry?;
            let path = directory.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                directories.push(path.clone());
            }
            paths.push(path);
        }
    }

    Ok(paths)
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

        let mut paths = scan(&root)?;
        paths.sort();
        fs::remove_dir_all(&root)?;

        let mut expected = vec![
            PathBuf::from("nested"),
            PathBuf::from("nested/empty"),
            PathBuf::from("nested/file.txt"),
        ];
        #[cfg(unix)]
        expected.push(PathBuf::from("nested/link"));
        expected.sort();
        assert_eq!(paths, expected);
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
