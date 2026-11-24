use rusqlite::{Connection, OptionalExtension};

use crate::error::StorageError;

#[derive(Debug)]
pub struct Root {
    pub id: i64,
    pub local_path: String,
    pub has_snapshot: bool,
}

// 임시: 루트가 하나뿐이라고 가정해 정렬을 보장하지 않는다. 공유 모델(share_id)을 넣을 때 다시 설계한다.
pub fn list_roots(conn: &Connection) -> Result<Vec<Root>, StorageError> {
    let mut stmt = conn.prepare("SELECT id, local_path, has_snapshot FROM roots")?;

    let rows = stmt.query_map([], |row| {
        Ok(Root {
            id: row.get(0)?,
            local_path: row.get(1)?,
            has_snapshot: row.get(2)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn insert_root(conn: &Connection, local_path: &str) -> Result<i64, StorageError> {
    conn.query_row(
        "INSERT INTO roots (local_path)
         VALUES (?1)
         ON CONFLICT(local_path) DO NOTHING RETURNING id",
        [local_path],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(StorageError::RootAlreadyExists)
}

pub fn get_root(conn: &Connection, root_id: i64) -> Result<Option<Root>, StorageError> {
    let root = conn
        .query_row(
            "
            SELECT id, local_path, has_snapshot
            FROM roots
            WHERE id = ?1
        ",
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

pub fn get_root_by_path(conn: &Connection, local_path: &str) -> Result<Option<Root>, StorageError> {
    let root = conn
        .query_row(
            "
            SELECT id, local_path, has_snapshot
            FROM roots
            WHERE local_path = ?1
        ",
            [local_path],
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

pub fn mark_root_has_snapshot(conn: &Connection, root_id: i64) -> Result<bool, StorageError> {
    let updated = conn.execute("UPDATE roots SET has_snapshot = 1 WHERE id = ?1", [root_id])?;
    Ok(updated != 0)
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

        assert!(mark_root_has_snapshot(&conn, root_id)?);

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
    fn returns_false_when_marking_missing_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        assert!(!mark_root_has_snapshot(&conn, 42)?);
        Ok(())
    }

    #[test]
    fn rolls_back_snapshot_marker_with_transaction() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;

        let tx = conn.transaction()?;
        assert!(mark_root_has_snapshot(&tx, root_id)?);
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

        assert!(matches!(result, Err(StorageError::RootAlreadyExists)));
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
    fn gets_registered_roots_by_exact_path() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let first_id = insert_root(&conn, "/example")?;
        let second_id = insert_root(&conn, "/example-other")?;
        mark_root_has_snapshot(&conn, second_id)?;

        for (path, id, has_snapshot) in [
            ("/example", first_id, false),
            ("/example-other", second_id, true),
        ] {
            let root = get_root_by_path(&conn, path)?.ok_or("root not found")?;
            assert_eq!(
                (root.id, root.local_path.as_str(), root.has_snapshot),
                (id, path, has_snapshot)
            );
        }
        Ok(())
    }

    #[test]
    fn returns_none_for_unregistered_root_path() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        insert_root(&conn, "/example")?;

        assert!(get_root_by_path(&conn, "/missing")?.is_none());
        assert!(get_root_by_path(&conn, "/exam")?.is_none());
        Ok(())
    }

    #[test]
    fn returns_none_for_missing_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let root = get_root(&conn, 42)?;

        assert!(root.is_none());
        Ok(())
    }

    #[test]
    fn lists_no_roots_in_an_empty_database() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        assert!(list_roots(&conn)?.is_empty());
        Ok(())
    }

    #[test]
    fn lists_the_registered_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;

        let roots = list_roots(&conn)?;

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, root_id);
        assert_eq!(roots[0].local_path, "/example");
        assert!(!roots[0].has_snapshot);
        Ok(())
    }

    #[test]
    fn lists_every_registered_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        insert_root(&conn, "/a")?;
        insert_root(&conn, "/b")?;

        // 순서는 보장하지 않으므로 경로만 모아서 비교한다.
        let mut paths: Vec<String> = list_roots(&conn)?
            .into_iter()
            .map(|root| root.local_path)
            .collect();
        paths.sort();

        assert_eq!(paths, ["/a", "/b"]);
        Ok(())
    }
}
