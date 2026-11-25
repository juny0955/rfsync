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

// SELECT 컬럼 목록. row_to_folder의 인덱스 순서와 맞아야 한다.
const FOLDER_COLUMNS: &str = "id, root_id, relative_path";

pub fn get_folders_by_root_id(
    conn: &Connection,
    root_id: i64,
) -> Result<Vec<Folder>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FOLDER_COLUMNS} FROM folders WHERE root_id = ?1"
    ))?;

    let rows = stmt.query_map([root_id], row_to_folder)?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_folders_with_under_path(
    conn: &Connection,
    root_id: i64,
    relative_path: &str,
) -> Result<Vec<Folder>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FOLDER_COLUMNS}
            FROM folders
            WHERE root_id = ?1
                AND (
                    relative_path = ?2
                    OR substr(relative_path, 1, length(?2) + 1) = ?2 || '/'
                )"
    ))?;

    let rows = stmt.query_map(params![root_id, relative_path], row_to_folder)?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_folder_by_path(
    conn: &Connection,
    root_id: i64,
    relative_path: &str,
) -> Result<Option<Folder>, StorageError> {
    let folder = conn
        .query_row(
            &format!(
                "SELECT {FOLDER_COLUMNS} FROM folders WHERE root_id = ?1 AND relative_path = ?2"
            ),
            params![root_id, relative_path],
            row_to_folder,
        )
        .optional()?;

    Ok(folder)
}

pub fn delete_folder_by_id(conn: &Connection, folder_id: i64) -> Result<bool, StorageError> {
    let deleted = conn.execute("DELETE FROM folders WHERE id = ?1", [folder_id])?;

    Ok(deleted > 0)
}

fn row_to_folder(row: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: row.get(0)?,
        root_id: row.get(1)?,
        relative_path: row.get(2)?,
    })
}

#[cfg(test)]
mod tests;
