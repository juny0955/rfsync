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
mod tests {
    use std::path::Path;

    use crate::open;

    use super::*;

    #[test]
    fn 페어링된_기기가_없으면_빈_목록을_반환한다() -> Result<(), StorageError> {
        let conn = open(Path::new(":memory:"))?;

        assert_eq!(list_devices(&conn)?, Vec::new());
        Ok(())
    }

    #[test]
    fn 페어링된_기기의_모든_열을_페어링_순서대로_나열한다() -> Result<(), StorageError> {
        let conn = open(Path::new(":memory:"))?;
        let listener_id = upsert_device(&conn, b"listener side", None, 100)?;
        let dialer_id = upsert_device(&conn, b"dialer side", Some("192.0.2.1:10000"), 200)?;
        conn.execute(
            "UPDATE devices SET name = 'Laptop' WHERE id = ?1",
            [dialer_id],
        )?;
        // 재페어링은 endpoint만 갱신하고 paired_at은 처음 값을 유지한다.
        upsert_device(&conn, b"listener side", Some("192.0.2.9:10000"), 300)?;

        assert_eq!(
            list_devices(&conn)?,
            vec![
                Device {
                    id: listener_id,
                    cert_der: b"listener side".to_vec(),
                    name: None,
                    endpoint: Some("192.0.2.9:10000".to_owned()),
                    paired_at: 100,
                },
                Device {
                    id: dialer_id,
                    cert_der: b"dialer side".to_vec(),
                    name: Some("Laptop".to_owned()),
                    endpoint: Some("192.0.2.1:10000".to_owned()),
                    paired_at: 200,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn 엔드포인트_없이_새_기기를_저장한다() -> Result<(), StorageError> {
        let conn = open(Path::new(":memory:"))?;
        let cert_der = b"device certificate";

        let id = upsert_device(&conn, cert_der, None, 100)?;

        let stored: (Vec<u8>, Option<String>, Option<String>, i64) = conn.query_row(
            "SELECT cert_der, name, endpoint, paired_at FROM devices WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        assert_eq!(stored, (cert_der.to_vec(), None, None, 100));
        Ok(())
    }

    #[test]
    fn 다시_페어링하면_기기_신원은_유지하고_알려진_엔드포인트만_갱신한다()
    -> Result<(), StorageError> {
        let conn = open(Path::new(":memory:"))?;
        let cert_der = b"device certificate";
        let id = upsert_device(&conn, cert_der, Some("192.0.2.1:10000"), 100)?;
        conn.execute("UPDATE devices SET name = 'Laptop' WHERE id = ?1", [id])?;

        for (endpoint, expected) in [
            (Some("192.0.2.2:10000"), "192.0.2.2:10000"),
            (None, "192.0.2.2:10000"),
        ] {
            assert_eq!(upsert_device(&conn, cert_der, endpoint, 200)?, id);
            let (name, stored_endpoint, paired_at): (String, String, i64) = conn.query_row(
                "SELECT name, endpoint, paired_at FROM devices WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(name, "Laptop");
            assert_eq!(stored_endpoint, expected);
            assert_eq!(paired_at, 100);
        }
        Ok(())
    }
}
