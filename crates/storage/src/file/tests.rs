use super::*;
use crate::{insert_folder, insert_root, open};
use std::error::Error;
use std::path::Path;

fn sample_file(folder_id: i64) -> File {
    File {
        id: -1,
        folder_id,
        name: "memo.txt".into(),
        size: 42,
        hash: "a".repeat(64),
        modified_secs: 123,
        modified_nanos: 456,
        deleted: false,
        version: Vec::new(),
    }
}

#[test]
fn 선택한_루트에서_폴더와_이름으로_파일을_조회한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let first_root = insert_root(&conn, "/first")?;
    let second_root = insert_root(&conn, "/second")?;
    let first_folder = insert_folder(&conn, first_root, "docs")?;
    let second_folder = insert_folder(&conn, second_root, "docs")?;
    insert_file(&conn, &sample_file(first_folder))?;
    let expected = sample_file(second_folder);
    let id = insert_file(&conn, &expected)?;

    let actual = get_file_by_folder_and_name(&conn, second_root, "docs", "memo.txt")?
        .ok_or("missing file")?;

    assert_eq!(actual.id, id);
    assert_eq!(actual.folder_id, expected.folder_id);
    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.size, expected.size);
    assert_eq!(actual.hash, expected.hash);
    assert_eq!(actual.modified_secs, expected.modified_secs);
    assert_eq!(actual.modified_nanos, expected.modified_nanos);
    Ok(())
}

#[test]
fn 파일이나_폴더나_루트가_없으면_없음을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root = insert_root(&conn, "/example")?;
    let folder = insert_folder(&conn, root, "")?;
    let id = insert_file(&conn, &sample_file(folder))?;

    assert_eq!(
        get_file_by_folder_and_name(&conn, root, "", "memo.txt")?.map(|f| f.id),
        Some(id)
    );
    assert!(get_file_by_folder_and_name(&conn, root, "", "missing")?.is_none());
    assert!(get_file_by_folder_and_name(&conn, root, "missing", "memo.txt")?.is_none());
    assert!(get_file_by_folder_and_name(&conn, -1, "", "memo.txt")?.is_none());
    Ok(())
}

#[test]
fn 버전_바이트를_저장하고_갱신한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let mut file = sample_file(folder_id);
    file.version = vec![1, 2, 3];
    file.id = insert_file(&conn, &file)?;
    let mut other = sample_file(folder_id);
    other.name = "other.txt".into();
    let other_id = insert_file(&conn, &other)?;

    let stored = get_file_by_folder_id_and_name(&conn, root_id, folder_id, "memo.txt")?
        .ok_or("missing file")?;
    assert_eq!(stored.version, vec![1, 2, 3]);

    file.version = vec![9, 8];
    assert!(update_file(&conn, file)?);

    let files = get_files_by_root_id(&conn, root_id)?;
    let updated = files
        .iter()
        .find(|f| f.name == "memo.txt")
        .ok_or("missing file")?;
    let other = files
        .iter()
        .find(|f| f.id == other_id)
        .ok_or("missing other")?;
    assert_eq!(updated.version, vec![9, 8]);
    assert!(other.version.is_empty());
    Ok(())
}

#[test]
fn 선택한_파일을_갱신한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let new_folder_id = insert_folder(&conn, root_id, "nested")?;
    let mut file = sample_file(folder_id);
    file.id = insert_file(&conn, &file)?;
    let other_id = insert_file(&conn, &sample_file(new_folder_id))?;
    file.folder_id = new_folder_id;
    file.name = "updated.txt".into();
    file.size = 99;
    file.hash = "b".repeat(64);
    file.modified_secs = 789;
    file.modified_nanos = 123;

    assert!(update_file(&conn, file)?);

    let files = get_files_by_root_id(&conn, root_id)?;
    let updated = files
        .iter()
        .find(|f| f.id != other_id)
        .ok_or("missing file")?;
    assert_eq!(
        (
            updated.folder_id,
            updated.name.as_str(),
            updated.size,
            updated.hash.as_str(),
            updated.modified_secs,
            updated.modified_nanos
        ),
        (
            new_folder_id,
            "updated.txt",
            99,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            789,
            123
        )
    );
    let other = files
        .iter()
        .find(|f| f.id == other_id)
        .ok_or("missing other file")?;
    assert_eq!((other.name.as_str(), other.size), ("memo.txt", 42));
    assert_eq!(files.len(), 2);
    Ok(())
}

#[test]
fn 갱신할_파일이_없으면_거짓을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    assert!(!update_file(&conn, sample_file(42))?);
    Ok(())
}

#[test]
fn 없는_폴더로_갱신하면_파일을_바꾸지_않고_거부한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let mut file = sample_file(folder_id);
    file.id = insert_file(&conn, &file)?;
    file.folder_id = 42;
    file.name = "changed.txt".into();

    let result = update_file(&conn, file);

    assert!(
        matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
    );
    let files = get_files_by_root_id(&conn, root_id)?;
    assert_eq!(files.len(), 1);
    assert_eq!(
        (files[0].folder_id, files[0].name.as_str()),
        (folder_id, "memo.txt")
    );
    Ok(())
}

#[test]
fn 파일_갱신을_트랜잭션으로_되돌린다() -> Result<(), Box<dyn Error>> {
    let mut conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let mut file = sample_file(folder_id);
    file.id = insert_file(&conn, &file)?;
    file.name = "changed.txt".into();

    let tx = conn.transaction()?;
    assert!(update_file(&tx, file)?);
    tx.rollback()?;

    let files = get_files_by_root_id(&conn, root_id)?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "memo.txt");
    Ok(())
}

#[test]
fn 선택한_파일만_삭제로_표시하고_그_버전만_갱신한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let mut file = sample_file(folder_id);
    file.version = vec![1];
    let file_id = insert_file(&conn, &file)?;
    let mut other = sample_file(folder_id);
    other.name = "other.txt".into();
    other.version = vec![7];
    let other_id = insert_file(&conn, &other)?;

    assert!(mark_file_deleted(&conn, file_id, &[2, 3])?);

    let files = get_files_by_root_id(&conn, root_id)?;
    let marked = files.iter().find(|f| f.id == file_id).ok_or("missing")?;
    let untouched = files.iter().find(|f| f.id == other_id).ok_or("missing")?;
    assert!(marked.deleted);
    assert_eq!(marked.version, vec![2, 3]);
    // 삭제 기록에도 지우기 직전 내용의 해시와 크기가 남는다.
    assert_eq!(
        (marked.hash.as_str(), marked.size),
        ("a".repeat(64).as_str(), 42)
    );
    assert!(!untouched.deleted);
    assert_eq!(untouched.version, vec![7]);
    assert!(!mark_file_deleted(&conn, 42, &[1])?);
    Ok(())
}

#[test]
fn 선택한_파일만_삭제한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let file_id = insert_file(&conn, &sample_file(folder_id))?;
    let mut other = sample_file(folder_id);
    other.name = "other.txt".into();
    let other_id = insert_file(&conn, &other)?;

    assert!(delete_file_by_id(&conn, file_id)?);

    let files = get_files_by_root_id(&conn, root_id)?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, other_id);
    Ok(())
}

#[test]
fn 파일이_없으면_거짓을_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    assert!(!delete_file_by_id(&conn, 42)?);
    Ok(())
}

#[test]
fn 파일_삭제를_트랜잭션으로_되돌린다() -> Result<(), Box<dyn Error>> {
    let mut conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let file_id = insert_file(&conn, &sample_file(folder_id))?;

    let tx = conn.transaction()?;
    assert!(delete_file_by_id(&tx, file_id)?);
    tx.rollback()?;

    let files = get_files_by_root_id(&conn, root_id)?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, file_id);
    Ok(())
}

#[test]
fn 선택한_루트의_파일만_조회한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let other_root_id = insert_root(&conn, "/other")?;
    let root_folder_id = insert_folder(&conn, root_id, "")?;
    let nested_folder_id = insert_folder(&conn, root_id, "nested")?;
    let other_folder_id = insert_folder(&conn, other_root_id, "")?;
    let first = sample_file(root_folder_id);
    let mut second = sample_file(nested_folder_id);
    second.name = "other.txt".into();
    second.size = 13;
    second.hash = "b".repeat(64);
    second.modified_secs = 789;
    second.modified_nanos = 123;
    let first_id = insert_file(&conn, &first)?;
    let second_id = insert_file(&conn, &second)?;
    insert_file(&conn, &sample_file(other_folder_id))?;

    let mut files = get_files_by_root_id(&conn, root_id)?;
    files.sort_by_key(|file| file.id);

    assert_eq!(files.len(), 2);
    for (actual, expected, id) in [
        (&files[0], &first, first_id),
        (&files[1], &second, second_id),
    ] {
        assert_eq!(actual.id, id);
        assert_eq!(actual.folder_id, expected.folder_id);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.hash, expected.hash);
        assert_eq!(actual.modified_secs, expected.modified_secs);
        assert_eq!(actual.modified_nanos, expected.modified_nanos);
    }
    assert!(get_files_by_root_id(&conn, 42)?.is_empty());
    Ok(())
}

#[test]
fn 메타데이터와_함께_파일을_추가하고_id를_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let file = sample_file(folder_id);

    let file_id = insert_file(&conn, &file)?;

    let saved: (i64, String, i64, String, i64, u32) = conn.query_row(
        "SELECT folder_id, name, fsize, hash, modified_secs, modified_nanos FROM files WHERE id = ?1",
        [file_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;
    assert_eq!(
        saved,
        (
            folder_id,
            file.name,
            file.size,
            file.hash,
            file.modified_secs,
            file.modified_nanos
        )
    );
    Ok(())
}

#[test]
fn 없는_폴더의_파일은_거부한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;

    let result = insert_file(&conn, &sample_file(42));

    assert!(
        matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY)
    );
    Ok(())
}

#[test]
fn 같은_폴더의_중복_파일은_거부한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root_id = insert_root(&conn, "/example")?;
    let folder_id = insert_folder(&conn, root_id, "")?;
    let file = sample_file(folder_id);
    insert_file(&conn, &file)?;

    let result = insert_file(&conn, &file);

    assert!(
        matches!(result, Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
    );
    Ok(())
}

#[test]
fn 루트와_폴더와_이름이_모두_일치하는_파일만_찾는다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root = insert_root(&conn, "/first")?;
    let other_root = insert_root(&conn, "/second")?;
    let folder = insert_folder(&conn, root, "docs")?;
    let other_folder = insert_folder(&conn, root, "other")?;
    let other_root_folder = insert_folder(&conn, other_root, "docs")?;
    let mut file = File {
        id: 0,
        folder_id: folder,
        name: "memo.txt".into(),
        size: 42,
        hash: "a".repeat(64),
        modified_secs: 123,
        modified_nanos: 456,
        deleted: false,
        version: Vec::new(),
    };
    let id = insert_file(&conn, &file)?;
    file.folder_id = other_folder;
    insert_file(&conn, &file)?;
    file.folder_id = other_root_folder;
    insert_file(&conn, &file)?;

    let actual =
        get_file_by_folder_id_and_name(&conn, root, folder, "memo.txt")?.ok_or("missing file")?;
    assert_eq!(
        (actual.id, actual.folder_id, actual.name),
        (id, folder, file.name)
    );
    assert_eq!(
        (
            actual.size,
            actual.hash,
            actual.modified_secs,
            actual.modified_nanos
        ),
        (
            file.size,
            file.hash,
            file.modified_secs,
            file.modified_nanos
        )
    );
    assert!(get_file_by_folder_id_and_name(&conn, other_root, folder, "memo.txt")?.is_none());
    assert!(get_file_by_folder_id_and_name(&conn, root, -1, "memo.txt")?.is_none());
    assert!(get_file_by_folder_id_and_name(&conn, root, folder, "missing")?.is_none());
    assert!(get_file_by_folder_id_and_name(&conn, -1, folder, "memo.txt")?.is_none());
    Ok(())
}

#[test]
fn 요청한_폴더의_파일만_반환한다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root = insert_root(&conn, "/first")?;
    let other_root = insert_root(&conn, "/second")?;
    let first = insert_folder(&conn, root, "first")?;
    let second = insert_folder(&conn, root, "second")?;
    let excluded = insert_folder(&conn, other_root, "first")?;
    let mut file = File {
        id: 0,
        folder_id: first,
        name: "memo.txt".into(),
        size: 42,
        hash: "a".repeat(64),
        modified_secs: 123,
        modified_nanos: 456,
        deleted: false,
        version: Vec::new(),
    };
    let first_id = insert_file(&conn, &file)?;
    file.folder_id = second;
    let second_id = insert_file(&conn, &file)?;
    file.folder_id = excluded;
    insert_file(&conn, &file)?;

    let mut files = get_files_in_folder_ids(&conn, &[first, second, first, -1])?;
    files.sort_by_key(|file| file.id);
    assert_eq!(files.len(), 2);
    for (actual, id, folder_id) in [(&files[0], first_id, first), (&files[1], second_id, second)] {
        assert_eq!(
            (actual.id, actual.folder_id, actual.name.as_str()),
            (id, folder_id, "memo.txt")
        );
        assert_eq!(
            (
                actual.size,
                actual.hash.as_str(),
                actual.modified_secs,
                actual.modified_nanos
            ),
            (42, file.hash.as_str(), 123, 456)
        );
    }
    Ok(())
}

#[test]
fn 폴더_id가_비었거나_없으면_파일을_반환하지_않는다() -> Result<(), Box<dyn Error>> {
    let conn = open(Path::new(":memory:"))?;
    let root = insert_root(&conn, "/first")?;
    let folder = insert_folder(&conn, root, "first")?;
    insert_file(
        &conn,
        &File {
            id: 0,
            folder_id: folder,
            name: "memo.txt".into(),
            size: 42,
            hash: "a".repeat(64),
            modified_secs: 123,
            modified_nanos: 456,
            deleted: false,
            version: Vec::new(),
        },
    )?;

    assert!(get_files_in_folder_ids(&conn, &[])?.is_empty());
    assert!(get_files_in_folder_ids(&conn, &[-1])?.is_empty());
    Ok(())
}
