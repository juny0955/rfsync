use rusqlite::{Connection, params};

use crate::error::StorageError;

pub struct File {
    pub id: i64,
    pub folder_id: i64,
    pub name: String,
    pub size: i64,
    pub hash: String,
    pub modified_secs: i64,
    pub modified_nanos: u32,
}

pub fn insert_file(conn: &Connection, file: &File) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO files
        (folder_id, name, fsize, hash, modified_secs, modified_nanos)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            file.folder_id,
            &file.name,
            file.size,
            &file.hash,
            file.modified_secs,
            file.modified_nanos
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_folder, insert_root, open};
    use std::error::Error;
    use std::path::Path;

    fn sample_file(folder_id: i64) -> File {
        File {
            id: -1,
            folder_id,
            name: "memo.txt".into(),
            size: 42,
            hash: "a".repeat(64),
            modified_secs: 123,
            modified_nanos: 456,
        }
    }

    #[test]
    fn inserts_file_with_metadata_and_returns_id() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let file = sample_file(folder_id);

        let file_id = insert_file(&conn, &file)?;

        let saved: (i64, String, i64, String, i64, u32) = conn.query_row(
            "SELECT folder_id, name, fsize, hash, modified_secs, modified_nanos FROM files WHERE id = ?1",
            [file_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )?;
        assert_eq!(
            saved,
            (
                folder_id,
                file.name,
                file.size,
                file.hash,
                file.modified_secs,
                file.modified_nanos
            )
        );
        Ok(())
    }

    #[test]
    fn rejects_file_for_missing_folder() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        let result = insert_file(&conn, &sample_file(42));

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
        );
        Ok(())
    }

    #[test]
    fn rejects_duplicate_file_in_same_folder() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let file = sample_file(folder_id);
        insert_file(&conn, &file)?;

        let result = insert_file(&conn, &file);

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
        );
        Ok(())
    }
}
