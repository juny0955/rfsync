use rusqlite::{Connection, OptionalExtension};

use crate::error::StorageError;

#[derive(Debug)]
pub struct Root {
    pub id: i64,
    pub local_path: String,
    pub has_snapshot: bool,
}

pub fn insert_root(conn: &Connection, local_path: &str) -> Result<i64, StorageError> {
    conn.execute("INSERT INTO roots (local_path) VALUES (?1)", [local_path])?;

    Ok(conn.last_insert_rowid())
}

pub fn get_root(conn: &Connection, root_id: i64) -> Result<Option<Root>, StorageError> {
    let root = conn
        .query_row(
            "SELECT id, local_path, has_snapshot
        FROM roots
        WHERE id = ?1",
            [root_id],
            |row| {
                Ok(Root {
                    id: row.get(0)?,
                    local_path: row.get(1)?,
                    has_snapshot: row.get(2)?,
                })
            },
        )
        .optional()?;

    Ok(root)
}

pub fn mark_root_has_snapshot(conn: &Connection, root_id: i64) -> Result<(), StorageError> {
    let updated = conn.execute("UPDATE roots SET has_snapshot = 1 WHERE id = ?1", [root_id])?;

    if updated == 0 {
        return Err(StorageError::RootNotFound(root_id));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open;
    use std::error::Error;
    use std::path::Path;

    #[test]
    fn marks_only_selected_root_as_having_snapshot() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let other_id = insert_root(&conn, "/other")?;

        mark_root_has_snapshot(&conn, root_id)?;

        assert!(
            get_root(&conn, root_id)?
                .ok_or("root not found")?
                .has_snapshot
        );
        assert!(
            !get_root(&conn, other_id)?
                .ok_or("root not found")?
                .has_snapshot
        );
        Ok(())
    }

    #[test]
    fn rejects_marking_missing_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let result = mark_root_has_snapshot(&conn, 42);

        assert!(matches!(result, Err(StorageError::RootNotFound(42))));
        Ok(())
    }

    #[test]
    fn rolls_back_snapshot_marker_with_transaction() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;

        let tx = conn.transaction()?;
        mark_root_has_snapshot(&tx, root_id)?;
        tx.rollback()?;

        assert!(
            !get_root(&conn, root_id)?
                .ok_or("root not found")?
                .has_snapshot
        );
        Ok(())
    }

    #[test]
    fn inserts_multiple_roots_with_distinct_ids_and_no_snapshots() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let first_id = insert_root(&conn, "/first")?;
        let second_id = insert_root(&conn, "/second")?;

        assert_ne!(first_id, second_id);
        for (id, expected_path) in [(first_id, "/first"), (second_id, "/second")] {
            let (path, has_snapshot): (String, i64) = conn.query_row(
                "SELECT local_path, has_snapshot FROM roots WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(path, expected_path);
            assert_eq!(has_snapshot, 0);
        }
        Ok(())
    }

    #[test]
    fn rejects_duplicate_root_without_changing_existing_entry() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;

        let result = insert_root(&conn, "/example");

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.code == rusqlite::ErrorCode::ConstraintViolation)
        );
        let (count, saved_id): (i64, i64) = conn.query_row(
            "SELECT count(*), id FROM roots WHERE local_path = ?1",
            ["/example"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((count, saved_id), (1, root_id));
        Ok(())
    }

    #[test]
    fn gets_existing_root_with_snapshot() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        conn.execute("UPDATE roots SET has_snapshot = 1 WHERE id = ?1", [root_id])?;

        let root = get_root(&conn, root_id)?.ok_or("root not found")?;

        assert_eq!(root.id, root_id);
        assert_eq!(root.local_path, "/example");
        assert!(root.has_snapshot);
        Ok(())
    }

    #[test]
    fn returns_none_for_missing_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let root = get_root(&conn, 42)?;

        assert!(root.is_none());
        Ok(())
    }
}
