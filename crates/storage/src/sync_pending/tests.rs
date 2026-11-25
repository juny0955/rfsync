use super::*;
use crate::{insert_root, open};
use std::{error::Error, path::Path};

fn pending(path: &str, hash_char: &str, deleted: bool, version: &[u8]) -> SyncPending {
    SyncPending {
        path: path.into(),
        hash: hash_char.repeat(64),
        deleted,
        version: version.to_vec(),
    }
}

#[test]
fn 넣은_행을_꺼내면_반환하고_삭제한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let row = pending("docs/a.txt", "a", false, &[1, 2, 3]);

    put_sync_pending(&conn, root_id, &row)?;

    assert_eq!(take_sync_pending(&conn, root_id, "docs/a.txt")?, Some(row));
    assert_eq!(take_sync_pending(&conn, root_id, "docs/a.txt")?, None);
    Ok(())
}

#[test]
fn 같은_경로에_넣으면_기존_행을_덮어쓴다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    put_sync_pending(&conn, root_id, &pending("a.txt", "a", false, &[1]))?;
    let newer = pending("a.txt", "b", true, &[2, 2]);

    put_sync_pending(&conn, root_id, &newer)?;

    assert_eq!(take_sync_pending(&conn, root_id, "a.txt")?, Some(newer));
    Ok(())
}

#[test]
fn 지정한_경로의_행만_삭제한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let other_root = insert_root(&conn, "/other")?;
    let keep = pending("keep.txt", "a", false, &[1]);
    put_sync_pending(&conn, root_id, &pending("gone.txt", "b", false, &[2]))?;
    put_sync_pending(&conn, root_id, &keep)?;
    put_sync_pending(&conn, other_root, &pending("gone.txt", "c", false, &[3]))?;

    assert!(delete_sync_pending(&conn, root_id, "gone.txt")?);
    assert!(!delete_sync_pending(&conn, root_id, "gone.txt")?);

    assert_eq!(take_sync_pending(&conn, root_id, "keep.txt")?, Some(keep));
    assert!(take_sync_pending(&conn, other_root, "gone.txt")?.is_some());
    Ok(())
}
