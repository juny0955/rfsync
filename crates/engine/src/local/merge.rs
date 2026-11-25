//! 내용이 같은데 버전이 서로 앞서지 않는 파일의 버전을 합쳐 저장한다.

use std::path::Path;

use rfsync_protocol::{Entry, VersionVector};
use rfsync_storage::{TransactionBehavior, error::StorageError};

use crate::error::EngineError;

/// 상대의 항목과 내용·삭제 여부가 같은 내 파일의 버전에 상대의 이력을 합쳐 저장한다. 바꾼 개수를 돌려준다.
///
/// 같은 내용을 각자 따로 등록하면 버전이 `{a:1}`과 `{b:1}`처럼 서로 앞서지 않는다. 합치지 않으면 이후 한쪽의
/// 삭제나 수정이 충돌로 보인다. 목록을 받은 뒤 그새 내용이 바뀐 파일은 건드리지 않는다(다음 비교에서 다시 본다).
///
/// 임시: 루트가 하나뿐이라고 가정해 첫 루트만 다룬다. `first_root_path`와 같은 가정이다.
pub(crate) fn merge_versions(db_path: &Path, entries: &[Entry]) -> Result<usize, EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let Some(root) = rfsync_storage::list_roots(&conn)?.into_iter().next() else {
        return Ok(0);
    };

    // 읽고 나서 쓰므로 처음부터 쓰기 잠금을 잡는다(감시자와 부딪혀 `database is locked`가 나지 않게).
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StorageError::from)?;
    let mut merged = 0;
    for entry in entries {
        let (folder, name) = entry.path.rsplit_once('/').unwrap_or(("", &entry.path));
        let Some(mut file) =
            rfsync_storage::get_file_by_folder_and_name(&tx, root.id, folder, name)?
        else {
            continue;
        };
        if file.deleted != entry.deleted
            || file.hash != blake3::Hash::from_bytes(entry.hash).to_hex().as_str()
        {
            continue;
        }

        let mut version = VersionVector::from_bytes(&file.version)?;
        let before = version.clone();
        version.merge(&entry.version);
        if version != before {
            file.version = version.to_bytes()?;
            rfsync_storage::update_file(&tx, file)?;
            merged += 1;
        }
    }
    tx.commit().map_err(StorageError::from)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use rfsync_protocol::DeviceId;

    use super::*;

    fn version(names: &[&str]) -> VersionVector {
        let mut version = VersionVector::default();
        for name in names {
            version.bump(DeviceId::from_cert(name.as_bytes()));
        }
        version
    }

    /// 파일 하나(`a.txt`, 내용 `hello`)를 `me`의 `{me:1}`로 등록한 DB.
    fn registered() -> (tempfile::TempDir, std::path::PathBuf, i64) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), "hello").unwrap();
        std::fs::write(root.join("sub/b.txt"), "world").unwrap();
        let db = dir.path().join("test.db");
        let root_id = crate::register_root(&root, &db, DeviceId::from_cert(b"me")).unwrap();
        (dir, db, root_id)
    }

    fn entry(path: &str, content: &str, deleted: bool, version: VersionVector) -> Entry {
        Entry {
            path: path.to_owned(),
            size: content.len() as u64,
            hash: *blake3::hash(content.as_bytes()).as_bytes(),
            modified_secs: 0,
            modified_nanos: 0,
            deleted,
            version,
        }
    }

    fn stored(db: &Path, root_id: i64, folder: &str, name: &str) -> VersionVector {
        let conn = rfsync_storage::open(db).unwrap();
        let file = rfsync_storage::get_file_by_folder_and_name(&conn, root_id, folder, name)
            .unwrap()
            .unwrap();
        VersionVector::from_bytes(&file.version).unwrap()
    }

    #[test]
    fn 같은_내용이면_상대의_이력을_합쳐_저장한다() {
        let (_dir, db, root_id) = registered();

        let merged = merge_versions(
            &db,
            &[
                entry("a.txt", "hello", false, version(&["peer"])),
                entry("sub/b.txt", "world", false, version(&["peer", "peer"])),
            ],
        )
        .unwrap();

        assert_eq!(merged, 2);
        let mut expected = version(&["me", "peer"]);
        assert_eq!(stored(&db, root_id, "", "a.txt"), expected);
        expected = version(&["me"]);
        expected.merge(&version(&["peer", "peer"]));
        assert_eq!(stored(&db, root_id, "sub", "b.txt"), expected);
    }

    #[test]
    fn 이미_상대의_이력을_포함하면_바꾸지_않는다() {
        let (_dir, db, root_id) = registered();

        let merged =
            merge_versions(&db, &[entry("a.txt", "hello", false, version(&["me"]))]).unwrap();

        assert_eq!(merged, 0);
        assert_eq!(stored(&db, root_id, "", "a.txt"), version(&["me"]));
    }

    #[test]
    fn 내용이_다르거나_삭제_여부가_다르거나_없는_파일은_건드리지_않는다() {
        let (_dir, db, root_id) = registered();

        let merged = merge_versions(
            &db,
            &[
                entry("a.txt", "different", false, version(&["peer"])),
                entry("sub/b.txt", "world", true, version(&["peer"])),
                entry("missing.txt", "x", false, version(&["peer"])),
            ],
        )
        .unwrap();

        assert_eq!(merged, 0);
        assert_eq!(stored(&db, root_id, "", "a.txt"), version(&["me"]));
        assert_eq!(stored(&db, root_id, "sub", "b.txt"), version(&["me"]));
    }

    #[test]
    fn 루트가_없으면_아무것도_하지_않는다() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");

        let merged = merge_versions(&db, &[entry("a.txt", "x", false, version(&["peer"]))]);

        assert_eq!(merged.unwrap(), 0);
    }
}
