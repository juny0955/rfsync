use std::{error::Error, io, path::Path};

use rusqlite::{Connection, TransactionBehavior};

pub fn open(path: &Path) -> Result<Connection, Box<dyn Error>> {
    let mut conn = Connection::open(path)?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    initialize_schema(&mut conn)?;

    Ok(conn)
}

fn initialize_schema(conn: &mut Connection) -> Result<(), Box<dyn Error>> {
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
                return Err(io::Error::other("스키마 버전이 없지만 비어있지 않은 DB입니다").into());
            }

            tx.execute_batch(include_str!("../sql/schema.sql"))?;
            tx.execute_batch("PRAGMA user_version = 1;")?;
        }
        1 => {}
        _ => {
            return Err(io::Error::other(format!("지원하지 않는 스키마 버전: {version}")).into());
        }
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        assert!(initialize_schema(&mut conn).is_err());
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

        assert!(initialize_schema(&mut conn).is_err());
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, 2);
        Ok(())
    }
}
