use std::{collections::HashMap, path::PathBuf};

use rfsync_protocol::Entry;
use rfsync_storage::DbConnection;

use crate::error::EngineError;

pub fn build_index(conn: &DbConnection, root_id: i64) -> Result<Vec<Entry>, EngineError> {
    let folders = rfsync_storage::get_folders_by_root_id(conn, root_id)?;
    let files = rfsync_storage::get_files_by_root_id(conn, root_id)?;

    let folder_paths: HashMap<i64, &str> = folders
        .iter()
        .map(|f| (f.id, f.relative_path.as_str()))
        .collect();

    let mut entries = Vec::new();
    for file in files {
        let parent = folder_paths
            .get(&file.folder_id)
            .ok_or_else(|| EngineError::ParentFolderNotFound(PathBuf::from(&file.name)))?;

        let path = if parent.is_empty() {
            file.name.clone()
        } else {
            format!("{parent}/{}", file.name)
        };

        entries.push(Entry {
            size: u64::try_from(file.size)?,
            hash: hash_from_hex(&file.hash)?,
            modified_secs: file.modified_secs,
            modified_nanos: file.modified_nanos,
            path,
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn hash_from_hex(hex: &str) -> Result<[u8; 32], EngineError> {
    let hash =
        blake3::Hash::from_hex(hex).map_err(|_| EngineError::InvalidHash(hex.to_string()))?;

    Ok(*hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 올바른_hex는_32바이트로_바뀐다() {
        let hex = "00".repeat(31) + "ff"; // 00 31번 + ff
        let bytes = hash_from_hex(&hex).unwrap();
        assert_eq!(bytes[0], 0);
        assert_eq!(bytes[31], 255);
    }

    #[test]
    fn 길이가_틀리거나_hex가_아니면_에러() {
        assert!(hash_from_hex("abc").is_err()); // 너무 짧음
        assert!(hash_from_hex(&"zz".repeat(32)).is_err()); // 64글자지만 hex가 아님
    }

    /// 스캔 대상 폴더와 DB를 서로 다른 임시 폴더에 두고 루트를 등록한다.
    /// DB가 스캔 대상 안에 있으면 DB 파일이 목록에 끼어들기 때문이다.
    fn register(
        files: &[(&str, &str)],
    ) -> (tempfile::TempDir, tempfile::TempDir, DbConnection, i64) {
        let root_dir = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();

        for (path, content) in files {
            let full = root_dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }

        let db = db_dir.path().join("test.db");
        let root_id = crate::register_root(root_dir.path(), &db).unwrap();
        let conn = rfsync_storage::open(&db).unwrap();

        (root_dir, db_dir, conn, root_id)
    }

    #[test]
    fn 저장된_파일이_경로와_크기와_해시를_가진_entry가_된다() {
        let (_root, _db, conn, root_id) = register(&[("a.txt", "hello"), ("sub/b.txt", "world!")]);

        let entries = build_index(&conn, root_id).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].size, 5);
        assert_eq!(entries[0].hash, *blake3::hash(b"hello").as_bytes());
        assert_eq!(entries[1].path, "sub/b.txt");
        assert_eq!(entries[1].size, 6);
        assert_eq!(entries[1].hash, *blake3::hash(b"world!").as_bytes());
    }

    #[test]
    fn 깊은_폴더의_파일도_슬래시로_이어진_경로가_된다() {
        let (_root, _db, conn, root_id) = register(&[("x/y/z.txt", "deep")]);

        let entries = build_index(&conn, root_id).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "x/y/z.txt");
    }

    #[test]
    fn entry는_경로순으로_정렬된다() {
        let (_root, _db, conn, root_id) = register(&[
            ("c.txt", "3"),
            ("a.txt", "1"),
            ("b/d.txt", "4"),
            ("b.txt", "2"),
        ]);

        let paths: Vec<String> = build_index(&conn, root_id)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();

        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        assert_eq!(paths.len(), 4);
    }

    #[test]
    fn 파일이_없는_루트는_빈_index가_된다() {
        let (_root, _db, conn, root_id) = register(&[]);

        assert!(build_index(&conn, root_id).unwrap().is_empty());
    }

    #[test]
    fn 빈_폴더만_있는_루트도_빈_index가_된다() {
        let root_dir = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(root_dir.path().join("empty")).unwrap();
        let db = db_dir.path().join("test.db");
        let root_id = crate::register_root(root_dir.path(), &db).unwrap();
        let conn = rfsync_storage::open(&db).unwrap();

        assert!(build_index(&conn, root_id).unwrap().is_empty());
    }
}
