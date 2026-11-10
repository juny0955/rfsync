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
        "
        INSERT INTO files (
            folder_id,
            name,
            fsize,
            hash,
            modified_secs,
            modified_nanos
        )
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

pub fn get_files_by_root_id(conn: &Connection, root_id: i64) -> Result<Vec<File>, StorageError> {
    let mut stmt = conn.prepare(
        "
            SELECT
                files.id,
                files.folder_id,
                files.name,
                files.fsize,
                files.hash,
                files.modified_secs,
                files.modified_nanos
            FROM files
            JOIN folders ON files.folder_id = folders.id
            WHERE folders.root_id = ?1
        ",
    )?;

    let rows = stmt.query_map([root_id], |row| {
        Ok(File {
            id: row.get(0)?,
            folder_id: row.get(1)?,
            name: row.get(2)?,
            size: row.get(3)?,
            hash: row.get(4)?,
            modified_secs: row.get(5)?,
            modified_nanos: row.get(6)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn delete_file_by_id(conn: &Connection, file_id: i64) -> Result<bool, StorageError> {
    let deleted = conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;

    Ok(deleted > 0)
}

pub fn update_file(conn: &Connection, file: File) -> Result<bool, StorageError> {
    let updated = conn.execute(
        "
            UPDATE files SET
                folder_id = ?1,
                name = ?2,
                fsize = ?3,
                hash = ?4,
                modified_secs = ?5,
                modified_nanos = ?6
            WHERE id = ?7
        ",
        params![
            file.folder_id,
            &file.name,
            file.size,
            &file.hash,
            file.modified_secs,
            file.modified_nanos,
            file.id,
        ],
    )?;

    Ok(updated > 0)
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
    fn updates_selected_file() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let new_folder_id = insert_folder(&conn, root_id, "nested")?;
        let mut file = sample_file(folder_id);
        file.id = insert_file(&conn, &file)?;
        let other_id = insert_file(&conn, &sample_file(new_folder_id))?;
        file.folder_id = new_folder_id;
        file.name = "updated.txt".into();
        file.size = 99;
        file.hash = "b".repeat(64);
        file.modified_secs = 789;
        file.modified_nanos = 123;

        assert!(update_file(&conn, file)?);

        let files = get_files_by_root_id(&conn, root_id)?;
        let updated = files
            .iter()
            .find(|f| f.id != other_id)
            .ok_or("missing file")?;
        assert_eq!(
            (
                updated.folder_id,
                updated.name.as_str(),
                updated.size,
                updated.hash.as_str(),
                updated.modified_secs,
                updated.modified_nanos
            ),
            (
                new_folder_id,
                "updated.txt",
                99,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                789,
                123
            )
        );
        let other = files
            .iter()
            .find(|f| f.id == other_id)
            .ok_or("missing other file")?;
        assert_eq!((other.name.as_str(), other.size), ("memo.txt", 42));
        assert_eq!(files.len(), 2);
        Ok(())
    }

    #[test]
    fn update_returns_false_when_file_does_not_exist() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        assert!(!update_file(&conn, sample_file(42))?);
        Ok(())
    }

    #[test]
    fn update_rejects_missing_folder_without_changing_file() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let mut file = sample_file(folder_id);
        file.id = insert_file(&conn, &file)?;
        file.folder_id = 42;
        file.name = "changed.txt".into();

        let result = update_file(&conn, file);

        assert!(
            matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
        );
        let files = get_files_by_root_id(&conn, root_id)?;
        assert_eq!(files.len(), 1);
        assert_eq!(
            (files[0].folder_id, files[0].name.as_str()),
            (folder_id, "memo.txt")
        );
        Ok(())
    }

    #[test]
    fn rolls_back_file_update() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let mut file = sample_file(folder_id);
        file.id = insert_file(&conn, &file)?;
        file.name = "changed.txt".into();

        let tx = conn.transaction()?;
        assert!(update_file(&tx, file)?);
        tx.rollback()?;

        let files = get_files_by_root_id(&conn, root_id)?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "memo.txt");
        Ok(())
    }

    #[test]
    fn deletes_only_selected_file() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let file_id = insert_file(&conn, &sample_file(folder_id))?;
        let mut other = sample_file(folder_id);
        other.name = "other.txt".into();
        let other_id = insert_file(&conn, &other)?;

        assert!(delete_file_by_id(&conn, file_id)?);

        let files = get_files_by_root_id(&conn, root_id)?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, other_id);
        Ok(())
    }

    #[test]
    fn returns_false_when_file_does_not_exist() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        assert!(!delete_file_by_id(&conn, 42)?);
        Ok(())
    }

    #[test]
    fn rolls_back_file_deletion() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "")?;
        let file_id = insert_file(&conn, &sample_file(folder_id))?;

        let tx = conn.transaction()?;
        assert!(delete_file_by_id(&tx, file_id)?);
        tx.rollback()?;

        let files = get_files_by_root_id(&conn, root_id)?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, file_id);
        Ok(())
    }

    #[test]
    fn gets_files_only_for_selected_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let other_root_id = insert_root(&conn, "/other")?;
        let root_folder_id = insert_folder(&conn, root_id, "")?;
        let nested_folder_id = insert_folder(&conn, root_id, "nested")?;
        let other_folder_id = insert_folder(&conn, other_root_id, "")?;
        let first = sample_file(root_folder_id);
        let mut second = sample_file(nested_folder_id);
        second.name = "other.txt".into();
        second.size = 13;
        second.hash = "b".repeat(64);
        second.modified_secs = 789;
        second.modified_nanos = 123;
        let first_id = insert_file(&conn, &first)?;
        let second_id = insert_file(&conn, &second)?;
        insert_file(&conn, &sample_file(other_folder_id))?;

        let mut files = get_files_by_root_id(&conn, root_id)?;
        files.sort_by_key(|file| file.id);

        assert_eq!(files.len(), 2);
        for (actual, expected, id) in [
            (&files[0], &first, first_id),
            (&files[1], &second, second_id),
        ] {
            assert_eq!(actual.id, id);
            assert_eq!(actual.folder_id, expected.folder_id);
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.size, expected.size);
            assert_eq!(actual.hash, expected.hash);
            assert_eq!(actual.modified_secs, expected.modified_secs);
            assert_eq!(actual.modified_nanos, expected.modified_nanos);
        }
        assert!(get_files_by_root_id(&conn, 42)?.is_empty());
        Ok(())
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
