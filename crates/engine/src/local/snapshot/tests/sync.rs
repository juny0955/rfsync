use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_protocol::{Causality, VersionVector};

use super::{
    create_initial_snapshot_for_test, sync_path_for_test, sync_root_for_test, test_device,
};
use crate::{error::EngineError, watch_root};

type Snapshot = (Vec<String>, Vec<(String, i64)>);

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
    db: PathBuf,
    root_id: i64,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rfsync-sync-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        let root = dir.join("root");
        let db = dir.join("snapshot.db");
        fs::create_dir_all(&root)?;
        let conn = rfsync_storage::open(&db)?;
        let root_id = rfsync_storage::insert_root(
            &conn,
            root.canonicalize()?.to_str().ok_or("non-UTF-8 test path")?,
        )?;
        Ok(Self {
            dir,
            root,
            db,
            root_id,
        })
    }

    fn snapshot(&self) -> Result<Snapshot, Box<dyn Error>> {
        let conn = rfsync_storage::open(&self.db)?;
        let mut folders: Vec<_> = rfsync_storage::get_folders_by_root_id(&conn, self.root_id)?
            .into_iter()
            .map(|folder| (folder.id, folder.relative_path))
            .collect();
        let mut files: Vec<_> = rfsync_storage::get_files_by_root_id(&conn, self.root_id)?
            .into_iter()
            .filter(|file| !file.deleted)
            .map(|file| {
                let parent = &folders
                    .iter()
                    .find(|(id, _)| *id == file.folder_id)
                    .unwrap()
                    .1;
                (
                    if parent.is_empty() {
                        file.name
                    } else {
                        format!("{parent}/{}", file.name)
                    },
                    file.size,
                )
            })
            .collect();
        let mut paths: Vec<_> = folders.drain(..).map(|(_, path)| path).collect();
        paths.sort();
        files.sort();
        Ok((paths, files))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.dir) {
            eprintln!("could not remove test directory: {error}");
        }
    }
}

#[test]
fn sync_root_applies_changes_and_preserves_unchanged_files() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir_all(f.root.join("removed/nested"))?;
    fs::write(f.root.join("removed/nested/old"), b"old")?;
    fs::write(f.root.join("modified"), b"old")?;
    fs::write(f.root.join("deleted"), b"old")?;
    fs::write(f.root.join("unchanged"), b"same")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::remove_dir_all(f.root.join("removed"))?;
    fs::remove_file(f.root.join("deleted"))?;
    fs::write(f.root.join("modified"), b"changed")?;
    fs::create_dir(f.root.join("added"))?;
    fs::write(f.root.join("added/new"), b"new")?;

    sync_root_for_test(f.root_id, &f.db)?;

    assert_eq!(
        f.snapshot()?,
        (
            vec![
                "".into(),
                "added".into(),
                "removed".into(),
                "removed/nested".into()
            ],
            vec![
                ("added/new".into(), 3),
                ("modified".into(), 7),
                ("unchanged".into(), 4)
            ]
        )
    );
    Ok(())
}

#[test]
fn sync_path_updates_only_selected_folder_tree() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir_all(f.root.join("docs/old"))?;
    fs::create_dir(f.root.join("docs-old"))?;
    fs::write(f.root.join("docs/old/file"), b"old")?;
    fs::write(f.root.join("docs/change"), b"old")?;
    fs::write(f.root.join("docs-old/keep"), b"outside")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::remove_dir_all(f.root.join("docs/old"))?;
    fs::write(f.root.join("docs/change"), b"changed")?;
    fs::create_dir(f.root.join("docs/new"))?;
    fs::write(f.root.join("docs/new/file"), b"new")?;
    fs::remove_dir_all(f.root.join("docs-old"))?;

    sync_path_for_test(f.root_id, &f.db, Path::new("docs"))?;

    assert_eq!(
        f.snapshot()?,
        (
            vec![
                "".into(),
                "docs".into(),
                "docs-old".into(),
                "docs/new".into(),
                "docs/old".into()
            ],
            vec![
                ("docs-old/keep".into(), 7),
                ("docs/change".into(), 7),
                ("docs/new/file".into(), 3)
            ]
        )
    );
    Ok(())
}

#[test]
fn sync_path_handles_file_creation_modification_and_deletion() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    let target = Path::new("file");
    fs::write(f.root.join(target), b"new")?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?.1, [("file".into(), 3)]);
    let conn = rfsync_storage::open(&f.db)?;
    let original_hash = rfsync_storage::get_files_by_root_id(&conn, f.root_id)?[0]
        .hash
        .clone();
    fs::write(f.root.join(target), b"alt")?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?.1, [("file".into(), 3)]);
    assert_ne!(
        rfsync_storage::get_files_by_root_id(&conn, f.root_id)?[0].hash,
        original_hash
    );
    drop(conn);
    fs::remove_file(f.root.join(target))?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert!(f.snapshot()?.1.is_empty());
    Ok(())
}

#[test]
fn sync_path_handles_file_folder_transitions_and_folder_deletion() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    let target = Path::new("item");
    fs::write(f.root.join(target), b"old")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::remove_file(f.root.join(target))?;
    fs::create_dir_all(f.root.join("item/nested"))?;
    fs::write(f.root.join("item/nested/file"), b"new")?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert_eq!(
        f.snapshot()?,
        (
            vec!["".into(), "item".into(), "item/nested".into()],
            vec![("item/nested/file".into(), 3)]
        )
    );
    fs::remove_dir_all(f.root.join(target))?;
    fs::write(f.root.join(target), b"file")?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert_eq!(
        f.snapshot()?,
        (
            vec!["".into(), "item".into(), "item/nested".into()],
            vec![("item".into(), 4)]
        )
    );
    fs::remove_file(f.root.join(target))?;
    fs::create_dir_all(f.root.join("item/nested"))?;
    fs::write(f.root.join("item/nested/file"), b"new")?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    fs::remove_dir_all(f.root.join(target))?;
    sync_path_for_test(f.root_id, &f.db, target)?;
    assert_eq!(
        f.snapshot()?,
        (vec!["".into(), "item".into(), "item/nested".into()], vec![])
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn sync_path_rolls_back_when_subtree_has_invalid_path() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("item"), b"original")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    let before = f.snapshot()?;
    fs::remove_file(f.root.join("item"))?;
    fs::create_dir(f.root.join("item"))?;
    fs::write(f.root.join("item/invalid\\name"), b"new")?;

    assert!(matches!(
        sync_path_for_test(f.root_id, &f.db, Path::new("item")),
        Err(EngineError::InvalidEntryPath(_))
    ));
    assert_eq!(f.snapshot()?, before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn sync_root_rolls_back_when_added_path_is_invalid() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("original"), b"original")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    let before = f.snapshot()?;
    fs::remove_file(f.root.join("original"))?;
    fs::write(f.root.join("invalid\\name"), b"new")?;

    assert!(matches!(
        sync_root_for_test(f.root_id, &f.db),
        Err(EngineError::InvalidEntryPath(_))
    ));
    assert_eq!(f.snapshot()?, before);
    Ok(())
}

#[test]
fn synchronization_rejects_uninitialized_root() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    assert!(
        matches!(watch_root(f.root_id, &f.db, super::test_device()), Err(EngineError::NotInitializedRoot(id)) if id == f.root_id)
    );
    Ok(())
}

#[test]
fn deleting_a_file_keeps_a_tombstone_and_recreating_it_revives_the_same_row()
-> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"hello")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    let conn = rfsync_storage::open(&f.db)?;
    let before = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("missing file")?;
    assert!(!before.deleted);

    fs::remove_file(f.root.join("memo"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    let tombstone = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("tombstone was removed")?;
    assert_eq!(tombstone.id, before.id);
    assert!(tombstone.deleted);
    assert_eq!(tombstone.hash, before.hash);

    // 같은 내용으로 다시 만들어도 되살아나야 한다.
    fs::write(f.root.join("memo"), b"hello")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    let revived = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("missing file")?;
    assert_eq!(revived.id, before.id);
    assert!(!revived.deleted);
    Ok(())
}

#[test]
fn deleting_a_folder_keeps_tombstones_of_files_inside() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir_all(f.root.join("docs/nested"))?;
    fs::write(f.root.join("docs/a"), b"a")?;
    fs::write(f.root.join("docs/nested/b"), b"bb")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;

    fs::remove_dir_all(f.root.join("docs"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs"))?;

    let conn = rfsync_storage::open(&f.db)?;
    for (folder, name) in [("docs", "a"), ("docs/nested", "b")] {
        let file = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, folder, name)?
            .ok_or("tombstone was removed")?;
        assert!(file.deleted);
    }
    // 같은 결과가 전체 동기화에서도 나와야 한다.
    fs::create_dir(f.root.join("docs"))?;
    fs::write(f.root.join("docs/a"), b"a")?;
    sync_root_for_test(f.root_id, &f.db)?;
    fs::remove_dir_all(f.root.join("docs"))?;
    sync_root_for_test(f.root_id, &f.db)?;
    let file = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "docs", "a")?
        .ok_or("tombstone was removed")?;
    assert!(file.deleted);
    assert!(f.snapshot()?.1.is_empty());
    Ok(())
}

/// 내가 `times`번 고친 파일의 version.
fn my_version(times: u32) -> VersionVector {
    let mut version = VersionVector::default();
    for _ in 0..times {
        version.bump(test_device());
    }
    version
}

fn stored_version(f: &Fixture, folder: &str, name: &str) -> Result<VersionVector, Box<dyn Error>> {
    let conn = rfsync_storage::open(&f.db)?;
    let file = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, folder, name)?
        .ok_or("missing file")?;
    Ok(VersionVector::from_bytes(&file.version)?)
}

#[test]
fn sync_root_gives_a_new_file_my_first_version() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("new"), b"hello")?;

    sync_root_for_test(f.root_id, &f.db)?;

    assert_eq!(stored_version(&f, "", "new")?, my_version(1));
    Ok(())
}

#[test]
fn sync_path_gives_a_new_file_my_first_version() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir(f.root.join("docs"))?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("docs/new"), b"hello")?;

    sync_path_for_test(f.root_id, &f.db, Path::new("docs/new"))?;

    assert_eq!(stored_version(&f, "docs", "new")?, my_version(1));
    Ok(())
}

#[test]
fn sync_root_raises_my_counter_when_content_changes() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;

    fs::write(f.root.join("memo"), b"two")?;
    sync_root_for_test(f.root_id, &f.db)?;
    assert_eq!(stored_version(&f, "", "memo")?, my_version(2));

    fs::write(f.root.join("memo"), b"three")?;
    sync_root_for_test(f.root_id, &f.db)?;
    assert_eq!(stored_version(&f, "", "memo")?, my_version(3));
    Ok(())
}

#[test]
fn sync_path_raises_my_counter_when_content_changes() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;

    fs::write(f.root.join("memo"), b"two")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    assert_eq!(stored_version(&f, "", "memo")?, my_version(2));

    fs::write(f.root.join("memo"), b"three")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    assert_eq!(stored_version(&f, "", "memo")?, my_version(3));
    Ok(())
}

#[test]
fn changing_only_the_modified_time_keeps_the_version() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("memo"), b"two")?;
    sync_root_for_test(f.root_id, &f.db)?;
    let conn = rfsync_storage::open(&f.db)?;
    let before = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("missing file")?;

    // 내용은 그대로 두고 수정 시각만 한 시간 앞으로 돌린다.
    let earlier = SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(f.root.join("memo"))?
        .set_modified(earlier)?;
    sync_root_for_test(f.root_id, &f.db)?;
    let after_root = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("missing file")?;
    // 수정 시각은 실제로 갱신됐지만 version은 그대로다.
    assert_ne!(after_root.modified_secs, before.modified_secs);
    assert_eq!(after_root.version, before.version);

    let earlier = earlier - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(f.root.join("memo"))?
        .set_modified(earlier)?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    let after_path = rfsync_storage::get_file_by_folder_and_name(&conn, f.root_id, "", "memo")?
        .ok_or("missing file")?;
    assert_ne!(after_path.modified_secs, after_root.modified_secs);
    assert_eq!(after_path.version, before.version);
    Ok(())
}

#[test]
fn recreating_a_deleted_file_moves_its_version_ahead() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("memo"), b"two")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    fs::remove_file(f.root.join("memo"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;
    let tombstone = stored_version(&f, "", "memo")?;

    // 같은 내용으로 다시 만들어도 삭제 기록보다 앞서야 상대가 새 변경으로 받는다.
    fs::write(f.root.join("memo"), b"two")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;

    let revived = stored_version(&f, "", "memo")?;
    assert_eq!(revived.compare(&tombstone), Causality::LocalAhead);
    Ok(())
}

#[test]
fn deleting_a_file_raises_my_counter_in_sync_path_and_sync_root() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("by_path"), b"a")?;
    fs::write(f.root.join("by_root"), b"b")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    // 초기 스냅샷이 1이고, 한 번 고쳐서 2로 만든다.
    fs::write(f.root.join("by_path"), b"aa")?;
    fs::write(f.root.join("by_root"), b"bb")?;
    sync_root_for_test(f.root_id, &f.db)?;

    fs::remove_file(f.root.join("by_path"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("by_path"))?;
    fs::remove_file(f.root.join("by_root"))?;
    sync_root_for_test(f.root_id, &f.db)?;

    assert_eq!(stored_version(&f, "", "by_path")?, my_version(3));
    assert_eq!(stored_version(&f, "", "by_root")?, my_version(3));
    Ok(())
}

#[test]
fn deleting_a_folder_raises_the_counter_of_every_file_inside() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir_all(f.root.join("docs/nested"))?;
    fs::write(f.root.join("docs/a"), b"a")?;
    fs::write(f.root.join("docs/nested/b"), b"b")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("docs/a"), b"aa")?;
    fs::write(f.root.join("docs/nested/b"), b"bb")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs"))?;

    fs::remove_dir_all(f.root.join("docs"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs"))?;

    assert_eq!(stored_version(&f, "docs", "a")?, my_version(3));
    assert_eq!(stored_version(&f, "docs/nested", "b")?, my_version(3));
    Ok(())
}

#[test]
fn an_already_deleted_file_keeps_its_version_on_later_events() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::create_dir(f.root.join("docs"))?;
    fs::write(f.root.join("docs/a"), b"a")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::remove_file(f.root.join("docs/a"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs/a"))?;
    let deleted = stored_version(&f, "docs", "a")?;
    assert_eq!(deleted, my_version(2));

    // 같은 경로와 부모 폴더의 이벤트가 다시 와도 삭제 기록은 앞서 나가지 않는다.
    sync_path_for_test(f.root_id, &f.db, Path::new("docs/a"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs"))?;
    sync_root_for_test(f.root_id, &f.db)?;

    assert_eq!(stored_version(&f, "docs", "a")?, deleted);
    Ok(())
}

#[test]
fn replacing_a_file_with_a_folder_raises_the_old_files_counter() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("item"), b"file")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;

    fs::remove_file(f.root.join("item"))?;
    fs::create_dir(f.root.join("item"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("item"))?;

    assert_eq!(stored_version(&f, "", "item")?, my_version(2));
    Ok(())
}

/// 상대에게서 받는 중이라는 기록을 남긴다. `content`는 받는 파일의 내용이다.
fn put_pending(
    f: &Fixture,
    path: &str,
    content: &[u8],
    deleted: bool,
    version: &VersionVector,
) -> Result<(), Box<dyn Error>> {
    let conn = rfsync_storage::open(&f.db)?;
    rfsync_storage::put_sync_pending(
        &conn,
        f.root_id,
        &rfsync_storage::SyncPending {
            path: path.to_owned(),
            hash: blake3::hash(content).to_hex().to_string(),
            deleted,
            version: version.to_bytes()?,
        },
    )?;
    Ok(())
}

fn pending_left(f: &Fixture, path: &str) -> Result<bool, Box<dyn Error>> {
    let conn = rfsync_storage::open(&f.db)?;
    Ok(rfsync_storage::take_sync_pending(&conn, f.root_id, path)?.is_some())
}

fn peer_version(times: u32) -> VersionVector {
    let mut version = VersionVector::default();
    for _ in 0..times {
        version.bump(rfsync_protocol::DeviceId::from_cert(b"peer"));
    }
    version
}

#[test]
fn a_received_file_keeps_the_peers_version_in_sync_path_and_sync_root() -> Result<(), Box<dyn Error>>
{
    let f = Fixture::new()?;
    fs::create_dir(f.root.join("docs"))?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;

    fs::write(f.root.join("docs/by_path"), b"one")?;
    put_pending(&f, "docs/by_path", b"one", false, &peer_version(2))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs/by_path"))?;
    fs::write(f.root.join("by_root"), b"two")?;
    put_pending(&f, "by_root", b"two", false, &peer_version(3))?;
    sync_root_for_test(f.root_id, &f.db)?;

    assert_eq!(stored_version(&f, "docs", "by_path")?, peer_version(2));
    assert_eq!(stored_version(&f, "", "by_root")?, peer_version(3));
    // 쓴 기록은 지워지고, 같은 파일에 이어 오는 이벤트는 version을 바꾸지 않는다.
    assert!(!pending_left(&f, "docs/by_path")?);
    assert!(!pending_left(&f, "by_root")?);
    sync_path_for_test(f.root_id, &f.db, Path::new("docs/by_path"))?;
    sync_root_for_test(f.root_id, &f.db)?;
    assert_eq!(stored_version(&f, "docs", "by_path")?, peer_version(2));
    Ok(())
}

#[test]
fn editing_a_received_file_again_merges_the_peers_history_and_raises_mine()
-> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    fs::write(f.root.join("memo"), b"received")?;
    put_pending(&f, "memo", b"received", false, &peer_version(1))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;

    // 받은 직후 내가 또 고쳤다.
    fs::write(f.root.join("memo"), b"mine")?;
    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;

    let mut expected = peer_version(1);
    expected.bump(test_device());
    assert_eq!(stored_version(&f, "", "memo")?, expected);
    Ok(())
}

#[test]
fn a_pending_record_that_does_not_match_the_content_is_left_in_place() -> Result<(), Box<dyn Error>>
{
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    // 받기로 한 내용(other)과 디스크의 내용이 달라, 받은 기록으로 볼 수 없다.
    fs::write(f.root.join("memo"), b"edited")?;
    put_pending(&f, "memo", b"other", false, &peer_version(1))?;

    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;

    // 내 수정으로만 기록하고, 받는 중일 수 있는 기록은 소비하지 않고 남긴다.
    assert_eq!(stored_version(&f, "", "memo")?, my_version(2));
    assert!(pending_left(&f, "memo")?);
    Ok(())
}

#[test]
fn a_delete_event_does_not_consume_a_pending_record_for_content() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("memo"), b"one")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    // 받는 도중 옛 파일이 지워지는 이벤트가 먼저 와도, 내용을 받는 기록은 남아야 한다.
    fs::remove_file(f.root.join("memo"))?;
    put_pending(&f, "memo", b"incoming", false, &peer_version(1))?;

    sync_path_for_test(f.root_id, &f.db, Path::new("memo"))?;

    assert!(pending_left(&f, "memo")?);
    Ok(())
}

#[test]
fn a_file_deleted_because_the_peer_deleted_it_keeps_the_peers_version() -> Result<(), Box<dyn Error>>
{
    let f = Fixture::new()?;
    fs::create_dir(f.root.join("docs"))?;
    fs::write(f.root.join("docs/by_path"), b"a")?;
    fs::write(f.root.join("by_root"), b"b")?;
    fs::create_dir(f.root.join("tree"))?;
    fs::write(f.root.join("tree/inner"), b"c")?;
    create_initial_snapshot_for_test(f.root_id, &f.db)?;
    let mut theirs = my_version(1);
    theirs.bump(rfsync_protocol::DeviceId::from_cert(b"peer"));

    put_pending(&f, "docs/by_path", b"a", true, &theirs)?;
    fs::remove_file(f.root.join("docs/by_path"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("docs/by_path"))?;
    put_pending(&f, "by_root", b"b", true, &theirs)?;
    fs::remove_file(f.root.join("by_root"))?;
    sync_root_for_test(f.root_id, &f.db)?;
    put_pending(&f, "tree/inner", b"c", true, &theirs)?;
    fs::remove_dir_all(f.root.join("tree"))?;
    sync_path_for_test(f.root_id, &f.db, Path::new("tree"))?;

    assert_eq!(stored_version(&f, "docs", "by_path")?, theirs);
    assert_eq!(stored_version(&f, "", "by_root")?, theirs);
    assert_eq!(stored_version(&f, "tree", "inner")?, theirs);
    assert!(!pending_left(&f, "docs/by_path")?);
    assert!(!pending_left(&f, "by_root")?);
    assert!(!pending_left(&f, "tree/inner")?);
    Ok(())
}
