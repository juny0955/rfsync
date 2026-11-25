use rusqlite::{Connection, OptionalExtension, params};

use crate::error::StorageError;

#[derive(Debug)]
pub struct Folder {
    pub id: i64,
    pub root_id: i64,
    pub relative_path: String,
}

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

pub fn get_folders_by_root_id(
    conn: &Connection,
    root_id: i64,
) -> Result<Vec<Folder>, StorageError> {
    let mut stmt =
        conn.prepare("SELECT id, root_id, relative_path FROM folders WHERE root_id = ?1")?;

    let rows = stmt.query_map([root_id], |row| {
        Ok(Folder {
            id: row.get(0)?,
            root_id: row.get(1)?,
            relative_path: row.get(2)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_folders_with_under_path(
    conn: &Connection,
    root_id: i64,
    relative_path: &str,
) -> Result<Vec<Folder>, StorageError> {
    let mut stmt = conn.prepare(
        "
            SELECT id, root_id, relative_path
            FROM folders
            WHERE root_id = ?1
                AND (
                    relative_path = ?2
                    OR substr(relative_path, 1, length(?2) + 1) = ?2 || '/'
                )
        ",
    )?;

    let rows = stmt.query_map(params![root_id, relative_path], |row| {
        Ok(Folder {
            id: row.get(0)?,
            root_id: row.get(1)?,
            relative_path: row.get(2)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_folder_by_path(
    conn: &Connection,
    root_id: i64,
    relative_path: &str,
) -> Result<Option<Folder>, StorageError> {
    let folder = conn
        .query_row(
            "
            SELECT id, root_id, relative_path
            FROM folders
            WHERE root_id = ?1 AND relative_path = ?2
            ",
            params![root_id, relative_path],
            |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    root_id: row.get(1)?,
                    relative_path: row.get(2)?,
                })
            },
        )
        .optional()?;

    Ok(folder)
}

pub fn delete_folder_by_id(conn: &Connection, folder_id: i64) -> Result<bool, StorageError> {
    let deleted = conn.execute("DELETE FROM folders WHERE id = ?1", [folder_id])?;

    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{File, get_files_by_root_id, insert_file, insert_root, open};
    use std::error::Error;
    use std::path::Path;

    #[test]
    fn gets_only_target_folder_and_descendants_in_selected_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/first")?;
        let other_root = insert_root(&conn, "/second")?;
        let root_folder = insert_folder(&conn, root_id, "")?;
        let docs = insert_folder(&conn, root_id, "docs")?;
        let work = insert_folder(&conn, root_id, "docs/work")?;
        let nested = insert_folder(&conn, root_id, "docs/work/2026")?;
        insert_folder(&conn, root_id, "docs-old")?;
        insert_folder(&conn, root_id, "docs-old/work")?;
        insert_folder(&conn, other_root, "docs/work")?;

        let mut folders = get_folders_with_under_path(&conn, root_id, "docs")?;
        folders.sort_by_key(|folder| folder.id);
        assert_eq!(
            folders
                .iter()
                .map(|f| (f.id, f.root_id, f.relative_path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (docs, root_id, "docs"),
                (work, root_id, "docs/work"),
                (nested, root_id, "docs/work/2026")
            ]
        );
        assert!(get_folders_with_under_path(&conn, root_id, "missing")?.is_empty());
        assert!(get_folders_with_under_path(&conn, -1, "docs")?.is_empty());
        let root_folders = get_folders_with_under_path(&conn, root_id, "")?;
        assert_eq!(root_folders.len(), 1);
        assert_eq!(root_folders[0].id, root_folder);
        Ok(())
    }

    #[test]
    fn treats_special_characters_in_folder_paths_literally() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        insert_folder(&conn, root_id, "문서%_")?;
        insert_folder(&conn, root_id, "문서%_/하위")?;
        insert_folder(&conn, root_id, "문서abc/하위")?;

        let mut paths: Vec<_> = get_folders_with_under_path(&conn, root_id, "문서%_")?
            .into_iter()
            .map(|folder| folder.relative_path)
            .collect();
        paths.sort();
        assert_eq!(paths, ["문서%_", "문서%_/하위"]);
        Ok(())
    }

    #[test]
    fn gets_folder_by_path_in_selected_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let first_root = insert_root(&conn, "/first")?;
        let second_root = insert_root(&conn, "/second")?;
        let root_folder_id = insert_folder(&conn, first_root, "")?;
        insert_folder(&conn, first_root, "docs")?;
        let folder_id = insert_folder(&conn, second_root, "docs")?;

        let folder = get_folder_by_path(&conn, second_root, "docs")?.ok_or("missing folder")?;
        assert_eq!(
            (folder.id, folder.root_id, folder.relative_path.as_str()),
            (folder_id, second_root, "docs")
        );
        assert_eq!(
            get_folder_by_path(&conn, first_root, "")?.map(|f| f.id),
            Some(root_folder_id)
        );
        assert!(get_folder_by_path(&conn, first_root, "missing")?.is_none());
        assert!(get_folder_by_path(&conn, second_root, "")?.is_none());
        assert!(get_folder_by_path(&conn, -1, "docs")?.is_none());
        Ok(())
    }

    #[test]
    fn deletes_folder_and_its_files_but_keeps_other_folders() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "photos")?;
        let nested_id = insert_folder(&conn, root_id, "photos/trip")?;
        for id in [folder_id, nested_id] {
            insert_file(
                &conn,
                &File {
                    id: 0,
                    folder_id: id,
                    name: "photo".into(),
                    size: 1,
                    hash: "a".repeat(64),
                    modified_secs: 0,
                    modified_nanos: 0,
                    deleted: false,
                    version: Vec::new(),
                },
            )?;
        }

        assert!(delete_folder_by_id(&conn, folder_id)?);

        let folders = get_folders_by_root_id(&conn, root_id)?;
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, nested_id);
        let files = get_files_by_root_id(&conn, root_id)?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].folder_id, nested_id);
        Ok(())
    }

    #[test]
    fn returns_false_when_folder_does_not_exist() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;

        assert!(!delete_folder_by_id(&conn, 42)?);
        Ok(())
    }

    #[test]
    fn rolls_back_folder_deletion() -> Result<(), Box<dyn Error>> {
        let mut conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let folder_id = insert_folder(&conn, root_id, "photos")?;
        insert_file(
            &conn,
            &File {
                id: 0,
                folder_id,
                name: "photo".into(),
                size: 1,
                hash: "a".repeat(64),
                modified_secs: 0,
                modified_nanos: 0,
                deleted: false,
                version: Vec::new(),
            },
        )?;

        let tx = conn.transaction()?;
        assert!(delete_folder_by_id(&tx, folder_id)?);
        tx.rollback()?;

        assert_eq!(get_folders_by_root_id(&conn, root_id)?[0].id, folder_id);
        assert_eq!(
            get_files_by_root_id(&conn, root_id)?[0].folder_id,
            folder_id
        );
        Ok(())
    }

    #[test]
    fn gets_only_folders_for_the_selected_root() -> Result<(), Box<dyn Error>> {
        let conn = open(Path::new(":memory:"))?;
        let root_id = insert_root(&conn, "/example")?;
        let other_root_id = insert_root(&conn, "/other")?;
        let root_folder_id = insert_folder(&conn, root_id, "")?;
        let nested_id = insert_folder(&conn, root_id, "photos/trip")?;
        insert_folder(&conn, other_root_id, "other")?;

        let mut folders = get_folders_by_root_id(&conn, root_id)?;
        folders.sort_by_key(|folder| folder.id);

        assert_eq!(folders.len(), 2);
        assert_eq!(
            (
                folders[0].id,
                folders[0].root_id,
                folders[0].relative_path.as_str()
            ),
            (root_folder_id, root_id, "")
        );
        assert_eq!(
            (
                folders[1].id,
                folders[1].root_id,
                folders[1].relative_path.as_str()
            ),
            (nested_id, root_id, "photos/trip")
        );
        assert!(get_folders_by_root_id(&conn, 42)?.is_empty());
        Ok(())
    }

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
