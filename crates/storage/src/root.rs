use rusqlite::{Connection, OptionalExtension};

use crate::error::StorageError;

#[derive(Debug)]
pub struct Root {
    pub id: i64,
    pub local_path: String,
    pub has_snapshot: bool,
}

// SELECT 컬럼 목록. row_to_root의 인덱스 순서와 맞아야 한다.
const ROOT_COLUMNS: &str = "id, local_path, has_snapshot";

// 임시: 루트가 하나뿐이라고 가정해 정렬을 보장하지 않는다. 공유 모델(share_id)을 넣을 때 다시 설계한다.
pub fn list_roots(conn: &Connection) -> Result<Vec<Root>, StorageError> {
    let mut stmt = conn.prepare(&format!("SELECT {ROOT_COLUMNS} FROM roots"))?;

    let rows = stmt.query_map([], row_to_root)?;

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
            &format!("SELECT {ROOT_COLUMNS} FROM roots WHERE id = ?1"),
            [root_id],
            row_to_root,
        )
        .optional()?;

    Ok(root)
}

pub fn get_root_by_path(conn: &Connection, local_path: &str) -> Result<Option<Root>, StorageError> {
    let root = conn
        .query_row(
            &format!("SELECT {ROOT_COLUMNS} FROM roots WHERE local_path = ?1"),
            [local_path],
            row_to_root,
        )
        .optional()?;

    Ok(root)
}

pub fn mark_root_has_snapshot(conn: &Connection, root_id: i64) -> Result<bool, StorageError> {
    let updated = conn.execute("UPDATE roots SET has_snapshot = 1 WHERE id = ?1", [root_id])?;
    Ok(updated != 0)
}

fn row_to_root(row: &rusqlite::Row<'_>) -> rusqlite::Result<Root> {
    Ok(Root {
        id: row.get(0)?,
        local_path: row.get(1)?,
        has_snapshot: row.get(2)?,
    })
}

#[cfg(test)]
mod tests;
