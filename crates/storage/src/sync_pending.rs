use rusqlite::{Connection, OptionalExtension, params};

use crate::error::StorageError;

/// 상대에게서 받아 적용 중인 변경. 감시자가 이 변경을 내 수정으로 오해하지 않게 하는 표시다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPending {
    pub path: String,
    pub hash: String,
    pub deleted: bool,
    pub version: Vec<u8>,
}

/// 받아 적용할 변경을 기록한다. 같은 경로의 기록이 있으면 덮어쓴다.
///
/// # Errors
///
/// 데이터베이스 쓰기에 실패하면 [`StorageError::Sqlite`]를 반환한다.
pub fn put_sync_pending(
    conn: &Connection,
    root_id: i64,
    pending: &SyncPending,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO sync_pending (root_id, path, hash, deleted, version)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(root_id, path) DO UPDATE
         SET hash = excluded.hash,
             deleted = excluded.deleted,
             version = excluded.version",
        params![
            root_id,
            &pending.path,
            &pending.hash,
            pending.deleted,
            &pending.version
        ],
    )?;
    Ok(())
}

/// 경로의 기록을 꺼내면서 지운다. 없으면 `None`이다.
///
/// 조회와 삭제를 한 문장으로 해서, 읽은 뒤 따로 지우는 사이에 다른 쪽이 끼어들 틈이 없다.
///
/// # Errors
///
/// 데이터베이스 접근에 실패하면 [`StorageError::Sqlite`]를 반환한다.
pub fn take_sync_pending(
    conn: &Connection,
    root_id: i64,
    path: &str,
) -> Result<Option<SyncPending>, StorageError> {
    Ok(conn
        .query_row(
            "DELETE FROM sync_pending
             WHERE root_id = ?1 AND path = ?2
             RETURNING path, hash, deleted, version",
            params![root_id, path],
            |row| {
                Ok(SyncPending {
                    path: row.get(0)?,
                    hash: row.get(1)?,
                    deleted: row.get(2)?,
                    version: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// 수신에 실패했을 때 남은 기록을 지운다. 지운 행이 있으면 `true`다.
///
/// # Errors
///
/// 데이터베이스 쓰기에 실패하면 [`StorageError::Sqlite`]를 반환한다.
pub fn delete_sync_pending(
    conn: &Connection,
    root_id: i64,
    path: &str,
) -> Result<bool, StorageError> {
    let deleted = conn.execute(
        "DELETE FROM sync_pending WHERE root_id = ?1 AND path = ?2",
        params![root_id, path],
    )?;
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_root, open};
    use std::{error::Error, path::Path};

    fn pending(path: &str, hash_char: &str, deleted: bool, version: &[u8]) -> SyncPending {
        SyncPending {
            path: path.into(),
            hash: hash_char.repeat(64),
            deleted,
            version: version.to_vec(),
        }
    }

    #[test]
    fn 넣은_행을_꺼내면_반환하고_삭제한다() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let row = pending("docs/a.txt", "a", false, &[1, 2, 3]);

        put_sync_pending(&conn, root_id, &row)?;

        assert_eq!(take_sync_pending(&conn, root_id, "docs/a.txt")?, Some(row));
        assert_eq!(take_sync_pending(&conn, root_id, "docs/a.txt")?, None);
        Ok(())
    }

    #[test]
    fn 같은_경로에_넣으면_기존_행을_덮어쓴다() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        put_sync_pending(&conn, root_id, &pending("a.txt", "a", false, &[1]))?;
        let newer = pending("a.txt", "b", true, &[2, 2]);

        put_sync_pending(&conn, root_id, &newer)?;

        assert_eq!(take_sync_pending(&conn, root_id, "a.txt")?, Some(newer));
        Ok(())
    }

    #[test]
    fn 지정한_경로의_행만_삭제한다() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let other_root = insert_root(&conn, "/other")?;
        let keep = pending("keep.txt", "a", false, &[1]);
        put_sync_pending(&conn, root_id, &pending("gone.txt", "b", false, &[2]))?;
        put_sync_pending(&conn, root_id, &keep)?;
        put_sync_pending(&conn, other_root, &pending("gone.txt", "c", false, &[3]))?;

        assert!(delete_sync_pending(&conn, root_id, "gone.txt")?);
        assert!(!delete_sync_pending(&conn, root_id, "gone.txt")?);

        assert_eq!(take_sync_pending(&conn, root_id, "keep.txt")?, Some(keep));
        assert!(take_sync_pending(&conn, other_root, "gone.txt")?.is_some());
        Ok(())
    }
}
