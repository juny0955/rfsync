use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn registers_canonical_root_path_and_returns_id() -> Result<(), Box<dyn Error>> {
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
        let root_id = rfsync_engine::register_root(&root.join("."), &db_path)?;
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
    assert_eq!(has_snapshot, 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn rejects_root_with_non_utf8_path() -> Result<(), Box<dyn Error>> {
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

    let result = rfsync_engine::register_root(&root, &test_dir.join("test.db"));

    assert!(
        matches!(result, Err(rfsync_engine::error::EngineError::InvalidRootPath(path)) if path == expected)
    );
    fs::remove_dir_all(test_dir)?;
    Ok(())
}
