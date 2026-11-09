use rusqlite::{Connection, params};

use crate::error::StorageError;

pub fn insert_folder(
    conn: &Connection,
    root_id: i64,
    relative_path: &str,
) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO folders (root_id, relative_path) VALUES (?1, ?2)",
        params![root_id, relative_path],
    )?;

    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_root, open};
    use std::error::Error;
    use std::path::Path;

    #[test]
    fn saves_folders_with_their_root_and_relative_path() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let other_root_id = insert_root(&conn, "/other")?;

        let root_folder_id = insert_folder(&conn, root_id, "")?;
        let folder_id = insert_folder(&conn, root_id, "photos/trip")?;
        let other_folder_id = insert_folder(&conn, other_root_id, "photos/trip")?;

        for (id, expected_root, expected_path) in [
            (root_folder_id, root_id, ""),
            (folder_id, root_id, "photos/trip"),
            (other_folder_id, other_root_id, "photos/trip"),
        ] {
            let (saved_root, saved_path): (i64, String) = conn.query_row(
                "SELECT root_id, relative_path FROM folders WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(
                (saved_root, saved_path.as_str()),
                (expected_root, expected_path)
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_duplicate_folder_in_same_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        insert_folder(&conn, root_id, "photos")?;

        let result = insert_folder(&conn, root_id, "photos");

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
        );
        Ok(())
    }

    #[test]
    fn rejects_folder_for_missing_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let result = insert_folder(&conn, 42, "photos");

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
        );
        Ok(())
    }

    #[test]
    fn rolls_back_folder_with_transaction() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;

        let tx = conn.transaction()?;
        insert_folder(&tx, root_id, "photos")?;
        tx.rollback()?;

        let count: i64 = conn.query_row("SELECT count(*) FROM folders", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    }
}
