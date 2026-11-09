use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use crate::error::StorageError;

pub mod error;

pub fn open(path: &Path) -> Result<Connection, StorageError> {
    let mut conn = Connection::open(path)?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    initialize_schema(&mut conn)?;

    Ok(conn)
}

pub fn insert_root(conn: &Connection, local_path: &str) -> Result<i64, StorageError> {
    conn.execute("INSERT INTO roots (local_path) VALUES (?1)", [local_path])?;

    Ok(conn.last_insert_rowid())
}

fn initialize_schema(conn: &mut Connection) -> Result<(), StorageError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    match version {
        0 => {
            let has_objects: bool = tx.query_row(
                "SELECT EXISTS (
                        SELECT 1 FROM sqlite_schema
                        WHERE name NOT LIKE 'sqlite_%'
                )",
                [],
                |row| row.get(0),
            )?;

            if has_objects {
                return Err(StorageError::UnversionedDatabase);
            }

            tx.execute_batch(include_str!("../sql/schema.sql"))?;
            tx.execute_batch("PRAGMA user_version = 1;")?;
        }
        1 => {}
        _ => {
            return Err(StorageError::UnsupportedSchemaVersion(version));
        }
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn initializes_new_database_with_foreign_keys() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        let table_count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('roots', 'folders', 'files')",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(version, 1);
        assert!(foreign_keys);
        assert_eq!(table_count, 3);
        assert!(
            conn.execute(
                "INSERT INTO folders (root_id, relative_path) VALUES (1, '')",
                []
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn preserves_data_when_database_is_reopened() -> Result<(), Box<dyn Error>> {
        let dir = std::env::temp_dir().join(format!(
            "rfsync-storage-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        std::fs::create_dir(&dir)?;
        let path = dir.join("storage.db");
        {
            let conn = open(&path)?;
            conn.execute("INSERT INTO roots (local_path) VALUES ('/example')", [])?;
        }

        {
            let conn = open(&path)?;
            let path: String =
                conn.query_row("SELECT local_path FROM roots", [], |row| row.get(0))?;
            assert_eq!(path, "/example");
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn rejects_unversioned_database_with_existing_tables() -> Result<(), Box<dyn Error>> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE existing (value TEXT); INSERT INTO existing VALUES ('keep');",
        )?;

        assert!(matches!(
            initialize_schema(&mut conn),
            Err(StorageError::UnversionedDatabase)
        ));
        let value: String = conn.query_row("SELECT value FROM existing", [], |row| row.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(value, "keep");
        assert_eq!(version, 0);
        Ok(())
    }

    #[test]
    fn rejects_unsupported_schema_version() -> Result<(), Box<dyn Error>> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA user_version = 2;")?;

        assert!(matches!(
            initialize_schema(&mut conn),
            Err(StorageError::UnsupportedSchemaVersion(2))
        ));
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, 2);
        Ok(())
    }
}
