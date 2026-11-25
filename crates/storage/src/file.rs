use rusqlite::{Connection, OptionalExtension, params};

use crate::error::StorageError;

pub struct File {
    pub id: i64,
    pub folder_id: i64,
    pub name: String,
    pub size: i64,
    pub hash: String,
    pub modified_secs: i64,
    pub modified_nanos: u32,
    pub deleted: bool,
    pub version: Vec<u8>,
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
            modified_nanos,
            deleted,
            version
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            file.folder_id,
            &file.name,
            file.size,
            &file.hash,
            file.modified_secs,
            file.modified_nanos,
            file.deleted,
            file.version
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

// SELECT 컬럼 목록. row_to_file의 인덱스 순서와 맞아야 한다.
const FILE_COLUMNS: &str = "
    files.id,
    files.folder_id,
    files.name,
    files.fsize,
    files.hash,
    files.modified_secs,
    files.modified_nanos,
    files.deleted,
    files.version";

const FILES_JOIN_FOLDERS: &str = "
    FROM files
    JOIN folders ON files.folder_id = folders.id";

pub fn get_files_by_root_id(conn: &Connection, root_id: i64) -> Result<Vec<File>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} {FILES_JOIN_FOLDERS} WHERE folders.root_id = ?1"
    ))?;

    let rows = stmt.query_map([root_id], row_to_file)?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_files_in_folder_ids(
    conn: &Connection,
    folder_ids: &[i64],
) -> Result<Vec<File>, StorageError> {
    if folder_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; folder_ids.len()].join(", ");
    let sql = format!("SELECT {FILE_COLUMNS} FROM files WHERE files.folder_id IN ({placeholders})");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(folder_ids.iter()), row_to_file)?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_file_by_folder_id_and_name(
    conn: &Connection,
    root_id: i64,
    folder_id: i64,
    file_name: &str,
) -> Result<Option<File>, StorageError> {
    let file = conn
        .query_row(
            &format!(
                "SELECT {FILE_COLUMNS} {FILES_JOIN_FOLDERS}
                WHERE folders.root_id = ?1
                    AND folders.id = ?2
                    AND files.name = ?3"
            ),
            params![root_id, folder_id, file_name],
            row_to_file,
        )
        .optional()?;

    Ok(file)
}

pub fn get_file_by_folder_and_name(
    conn: &Connection,
    root_id: i64,
    folder_path: &str,
    file_name: &str,
) -> Result<Option<File>, StorageError> {
    let file = conn
        .query_row(
            &format!(
                "SELECT {FILE_COLUMNS} {FILES_JOIN_FOLDERS}
                WHERE folders.root_id = ?1
                    AND folders.relative_path = ?2
                    AND files.name = ?3"
            ),
            params![root_id, folder_path, file_name],
            row_to_file,
        )
        .optional()?;

    Ok(file)
}

pub fn delete_file_by_id(conn: &Connection, file_id: i64) -> Result<bool, StorageError> {
    let deleted = conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;

    Ok(deleted > 0)
}

/// 파일을 삭제 표시하면서 버전도 함께 바꾼다. 삭제도 한 번의 변경이므로 버전이 따라가야 한다.
pub fn mark_file_deleted(
    conn: &Connection,
    file_id: i64,
    version: &[u8],
) -> Result<bool, StorageError> {
    let deleted = conn.execute(
        "UPDATE files SET deleted = 1, version = ?1 WHERE id = ?2",
        params![version, file_id],
    )?;

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
                modified_nanos = ?6,
                deleted = ?7,
                version = ?8
            WHERE id = ?9
        ",
        params![
            file.folder_id,
            &file.name,
            file.size,
            &file.hash,
            file.modified_secs,
            file.modified_nanos,
            file.deleted,
            file.version,
            file.id,
        ],
    )?;

    Ok(updated > 0)
}

fn row_to_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<File> {
    Ok(File {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        name: row.get(2)?,
        size: row.get(3)?,
        hash: row.get(4)?,
        modified_secs: row.get(5)?,
        modified_nanos: row.get(6)?,
        deleted: row.get(7)?,
        version: row.get(8)?,
    })
}

#[cfg(test)]
mod tests;
