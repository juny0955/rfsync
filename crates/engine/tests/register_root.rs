use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rfsync_engine::error::EngineError;
use rfsync_storage::error::StorageError;

#[test]
fn 정규화된_루트_경로를_초기_스냅샷과_함께_등록하고_id를_반환한다() -> Result<(), Box<dyn Error>>
{
    let test_dir = std::env::temp_dir().join(format!(
        "rfsync-core-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&test_dir)?;
    let root = test_dir.join("root");
    let db_path = test_dir.join("test.db");
    fs::create_dir(&root)?;

    let result = (|| -> Result<_, Box<dyn Error>> {
        let expected_path = root.canonicalize()?;
        let root_id = rfsync_engine::register_root(
            &root.join("."),
            &db_path,
            rfsync_engine::DeviceId::from_cert(b"test"),
        )?;
        let conn = rfsync_storage::open(&db_path)?;
        let (saved_path, has_snapshot): (String, i64) = conn.query_row(
            "SELECT local_path, has_snapshot FROM roots WHERE id = ?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((expected_path, saved_path, has_snapshot))
    })();

    fs::remove_dir_all(test_dir)?;
    let (expected_path, saved_path, has_snapshot) = result?;
    assert_eq!(Path::new(&saved_path), expected_path);
    assert_eq!(has_snapshot, 1);
    Ok(())
}

#[test]
fn 이미_등록된_정규화_경로는_거부한다() -> Result<(), Box<dyn Error>> {
    let test_dir = std::env::temp_dir().join(format!(
        "rfsync-duplicate-root-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&test_dir)?;
    let root = test_dir.join("root");
    let db_path = test_dir.join("test.db");
    fs::create_dir(&root)?;
    rfsync_engine::register_root(&root, &db_path, rfsync_engine::DeviceId::from_cert(b"test"))?;

    let result = rfsync_engine::register_root(
        &root.join("."),
        &db_path,
        rfsync_engine::DeviceId::from_cert(b"test"),
    );

    assert!(matches!(
        result,
        Err(EngineError::Storage(StorageError::RootAlreadyExists))
    ));
    fs::remove_dir_all(test_dir)?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn UTF8이_아닌_경로의_루트는_거부한다() -> Result<(), Box<dyn Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let test_dir = std::env::temp_dir().join(format!(
        "rfsync-invalid-root-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&test_dir)?;
    let root = test_dir.join(OsString::from_vec(vec![0xff]));
    fs::create_dir(&root)?;
    let expected = root.canonicalize()?;

    let result = rfsync_engine::register_root(
        &root,
        &test_dir.join("test.db"),
        rfsync_engine::DeviceId::from_cert(b"test"),
    );

    assert!(matches!(result, Err(EngineError::InvalidRootPath(path)) if path == expected));
    fs::remove_dir_all(test_dir)?;
    Ok(())
}
