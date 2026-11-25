use rusqlite::{Connection, params};

use crate::error::StorageError;

/// `devices` 테이블의 한 행에 해당하는 페어링된 기기.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: i64,
    pub cert_der: Vec<u8>,
    /// 사용자가 붙인 기기 이름. 지정하지 않았으면 `None`이다.
    pub name: Option<String>,
    /// 접속할 주소. 상대가 우리에게 접속해 오기만 하는 기기는 `None`이다.
    pub endpoint: Option<String>,
    /// 처음 페어링한 시각(Unix 초).
    pub paired_at: i64,
}

/// Stores a device by certificate DER and returns its ID.
///
/// `paired_at` is the first pairing time in Unix seconds. For an existing
/// device, a supplied endpoint replaces the stored one and `None` preserves it.
///
/// # Errors
///
/// Returns [`StorageError::Sqlite`] if the database write fails.
pub fn upsert_device(
    conn: &Connection,
    cert_der: &[u8],
    endpoint: Option<&str>,
    paired_at: i64,
) -> Result<i64, StorageError> {
    Ok(conn.query_row(
        "INSERT INTO devices (cert_der, endpoint, paired_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(cert_der) DO UPDATE
         SET endpoint = COALESCE(excluded.endpoint, devices.endpoint)
         RETURNING id",
        params![cert_der, endpoint, paired_at],
        |row| row.get(0),
    )?)
}

/// 페어링된 모든 기기를 ID 순서로 반환한다.
///
/// # Errors
///
/// 데이터베이스 읽기에 실패하면 [`StorageError::Sqlite`]를 반환한다.
pub fn list_devices(conn: &Connection) -> Result<Vec<Device>, StorageError> {
    let mut stmt =
        conn.prepare("SELECT id, cert_der, name, endpoint, paired_at FROM devices ORDER BY id")?;
    let devices = stmt
        .query_map([], |row| {
            Ok(Device {
                id: row.get(0)?,
                cert_der: row.get(1)?,
                name: row.get(2)?,
                endpoint: row.get(3)?,
                paired_at: row.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(devices)
}

#[cfg(test)]
mod tests;
