use rusqlite::{Connection, params};

use crate::error::StorageError;

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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::open;

    use super::*;

    #[test]
    fn stores_new_device_without_endpoint() -> Result<(), StorageError> {
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
    fn repairing_keeps_device_identity_and_updates_only_known_endpoints() -> Result<(), StorageError>
    {
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
