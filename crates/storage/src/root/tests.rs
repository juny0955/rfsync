use super::*;
use crate::open;
use std::error::Error;
use std::path::Path;

#[test]
fn 선택한_루트만_스냅샷이_있다고_표시한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let other_id = insert_root(&conn, "/other")?;

    assert!(mark_root_has_snapshot(&conn, root_id)?);

    assert!(
        get_root(&conn, root_id)?
            .ok_or("root not found")?
            .has_snapshot
    );
    assert!(
        !get_root(&conn, other_id)?
            .ok_or("root not found")?
            .has_snapshot
    );
    Ok(())
}

#[test]
fn 없는_루트를_표시하면_거짓을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    assert!(!mark_root_has_snapshot(&conn, 42)?);
    Ok(())
}

#[test]
fn 스냅샷_표시를_트랜잭션으로_되돌린다() -> Result<(), Box<dyn Error>> {
    let mut conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;

    let tx = conn.transaction()?;
    assert!(mark_root_has_snapshot(&tx, root_id)?);
    tx.rollback()?;

    assert!(
        !get_root(&conn, root_id)?
            .ok_or("root not found")?
            .has_snapshot
    );
    Ok(())
}

#[test]
fn 여러_루트를_서로_다른_id로_스냅샷_없이_추가한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    let first_id = insert_root(&conn, "/first")?;
    let second_id = insert_root(&conn, "/second")?;

    assert_ne!(first_id, second_id);
    for (id, expected_path) in [(first_id, "/first"), (second_id, "/second")] {
        let (path, has_snapshot): (String, i64) = conn.query_row(
            "SELECT local_path, has_snapshot FROM roots WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(path, expected_path);
        assert_eq!(has_snapshot, 0);
    }
    Ok(())
}

#[test]
fn 중복_루트는_기존_항목을_바꾸지_않고_거부한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;

    let result = insert_root(&conn, "/example");

    assert!(matches!(result, Err(StorageError::RootAlreadyExists)));
    let (count, saved_id): (i64, i64) = conn.query_row(
        "SELECT count(*), id FROM roots WHERE local_path = ?1",
        ["/example"],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!((count, saved_id), (1, root_id));
    Ok(())
}

#[test]
fn 스냅샷이_있는_기존_루트를_조회한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    conn.execute("UPDATE roots SET has_snapshot = 1 WHERE id = ?1", [root_id])?;

    let root = get_root(&conn, root_id)?.ok_or("root not found")?;

    assert_eq!(root.id, root_id);
    assert_eq!(root.local_path, "/example");
    assert!(root.has_snapshot);
    Ok(())
}

#[test]
fn 정확한_경로로_등록된_루트를_조회한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let first_id = insert_root(&conn, "/example")?;
    let second_id = insert_root(&conn, "/example-other")?;
    mark_root_has_snapshot(&conn, second_id)?;

    for (path, id, has_snapshot) in [
        ("/example", first_id, false),
        ("/example-other", second_id, true),
    ] {
        let root = get_root_by_path(&conn, path)?.ok_or("root not found")?;
        assert_eq!(
            (root.id, root.local_path.as_str(), root.has_snapshot),
            (id, path, has_snapshot)
        );
    }
    Ok(())
}

#[test]
fn 등록되지_않은_루트_경로는_없음을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    insert_root(&conn, "/example")?;

    assert!(get_root_by_path(&conn, "/missing")?.is_none());
    assert!(get_root_by_path(&conn, "/exam")?.is_none());
    Ok(())
}

#[test]
fn 없는_루트는_없음을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    let root = get_root(&conn, 42)?;

    assert!(root.is_none());
    Ok(())
}

#[test]
fn 빈_데이터베이스에서는_루트가_없다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    assert!(list_roots(&conn)?.is_empty());
    Ok(())
}

#[test]
fn 등록된_루트를_나열한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;

    let roots = list_roots(&conn)?;

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, root_id);
    assert_eq!(roots[0].local_path, "/example");
    assert!(!roots[0].has_snapshot);
    Ok(())
}

#[test]
fn 등록된_모든_루트를_나열한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    insert_root(&conn, "/a")?;
    insert_root(&conn, "/b")?;

    // 순서는 보장하지 않으므로 경로만 모아서 비교한다.
    let mut paths: Vec<String> = list_roots(&conn)?
        .into_iter()
        .map(|root| root.local_path)
        .collect();
    paths.sort();

    assert_eq!(paths, ["/a", "/b"]);
    Ok(())
}
