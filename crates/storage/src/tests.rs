use super::*;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn 새_데이터베이스를_외래_키와_함께_초기화한다() -> Result<(), Box<dyn Error>> {
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
fn 데이터베이스를_다시_열어도_데이터를_보존한다() -> Result<(), Box<dyn Error>> {
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
        let path: String = conn.query_row("SELECT local_path FROM roots", [], |row| row.get(0))?;
        assert_eq!(path, "/example");
    }
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn 테이블이_있는_버전_없는_데이터베이스는_거부한다() -> Result<(), Box<dyn Error>> {
    let mut conn = DbConnection::open_in_memory()?;
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
fn 지원하지_않는_스키마_버전은_거부한다() -> Result<(), Box<dyn Error>> {
    let mut conn = DbConnection::open_in_memory()?;
    conn.execute_batch("PRAGMA user_version = 2;")?;

    assert!(matches!(
        initialize_schema(&mut conn),
        Err(StorageError::UnsupportedSchemaVersion(2))
    ));
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 2);
    Ok(())
}
