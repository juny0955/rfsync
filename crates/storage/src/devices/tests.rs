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
fn 다시_페어링하면_기기_신원은_유지하고_알려진_엔드포인트만_갱신한다() -> Result<(), StorageError> {
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
