use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_engine::{
    create_initial_snapshot, error::EngineError, register_root, sync_path, sync_root,
};

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
        let root_id = register_root(&root, &db)?;
        Ok(Self {
            dir,
            root,
            db,
            root_id,
        })
    }

    fn snapshot(&self) -> Result<(Vec<String>, Vec<(String, i64)>), Box<dyn Error>> {
        let conn = rfsync_storage::open(&self.db)?;
        let mut folders: Vec<_> = rfsync_storage::get_folders_by_root_id(&conn, self.root_id)?
            .into_iter()
            .map(|folder| (folder.id, folder.relative_path))
            .collect();
        let mut files: Vec<_> = rfsync_storage::get_files_by_root_id(&conn, self.root_id)?
            .into_iter()
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
    create_initial_snapshot(f.root_id, &f.db)?;
    fs::remove_dir_all(f.root.join("removed"))?;
    fs::remove_file(f.root.join("deleted"))?;
    fs::write(f.root.join("modified"), b"changed")?;
    fs::create_dir(f.root.join("added"))?;
    fs::write(f.root.join("added/new"), b"new")?;

    sync_root(f.root_id, &f.db)?;

    assert_eq!(
        f.snapshot()?,
        (
            vec!["".into(), "added".into()],
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
    create_initial_snapshot(f.root_id, &f.db)?;
    fs::remove_dir_all(f.root.join("docs/old"))?;
    fs::write(f.root.join("docs/change"), b"changed")?;
    fs::create_dir(f.root.join("docs/new"))?;
    fs::write(f.root.join("docs/new/file"), b"new")?;
    fs::remove_dir_all(f.root.join("docs-old"))?;

    sync_path(f.root_id, &f.db, Path::new("docs"))?;

    assert_eq!(
        f.snapshot()?,
        (
            vec![
                "".into(),
                "docs".into(),
                "docs-old".into(),
                "docs/new".into()
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
    create_initial_snapshot(f.root_id, &f.db)?;
    let target = Path::new("file");
    fs::write(f.root.join(target), b"new")?;
    sync_path(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?.1, [("file".into(), 3)]);
    let conn = rfsync_storage::open(&f.db)?;
    let original_hash = rfsync_storage::get_files_by_root_id(&conn, f.root_id)?[0]
        .hash
        .clone();
    fs::write(f.root.join(target), b"alt")?;
    sync_path(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?.1, [("file".into(), 3)]);
    assert_ne!(
        rfsync_storage::get_files_by_root_id(&conn, f.root_id)?[0].hash,
        original_hash
    );
    drop(conn);
    fs::remove_file(f.root.join(target))?;
    sync_path(f.root_id, &f.db, target)?;
    assert!(f.snapshot()?.1.is_empty());
    Ok(())
}

#[test]
fn sync_path_handles_file_folder_transitions_and_folder_deletion() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    let target = Path::new("item");
    fs::write(f.root.join(target), b"old")?;
    create_initial_snapshot(f.root_id, &f.db)?;
    fs::remove_file(f.root.join(target))?;
    fs::create_dir_all(f.root.join("item/nested"))?;
    fs::write(f.root.join("item/nested/file"), b"new")?;
    sync_path(f.root_id, &f.db, target)?;
    assert_eq!(
        f.snapshot()?,
        (
            vec!["".into(), "item".into(), "item/nested".into()],
            vec![("item/nested/file".into(), 3)]
        )
    );
    fs::remove_dir_all(f.root.join(target))?;
    fs::write(f.root.join(target), b"file")?;
    sync_path(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?, (vec!["".into()], vec![("item".into(), 4)]));
    fs::remove_file(f.root.join(target))?;
    fs::create_dir_all(f.root.join("item/nested"))?;
    fs::write(f.root.join("item/nested/file"), b"new")?;
    sync_path(f.root_id, &f.db, target)?;
    fs::remove_dir_all(f.root.join(target))?;
    sync_path(f.root_id, &f.db, target)?;
    assert_eq!(f.snapshot()?, (vec!["".into()], vec![]));
    Ok(())
}

#[cfg(unix)]
#[test]
fn sync_path_rolls_back_when_subtree_has_invalid_path() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    fs::write(f.root.join("item"), b"original")?;
    create_initial_snapshot(f.root_id, &f.db)?;
    let before = f.snapshot()?;
    fs::remove_file(f.root.join("item"))?;
    fs::create_dir(f.root.join("item"))?;
    fs::write(f.root.join("item/invalid\\name"), b"new")?;

    assert!(matches!(
        sync_path(f.root_id, &f.db, Path::new("item")),
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
    create_initial_snapshot(f.root_id, &f.db)?;
    let before = f.snapshot()?;
    fs::remove_file(f.root.join("original"))?;
    fs::write(f.root.join("invalid\\name"), b"new")?;

    assert!(matches!(
        sync_root(f.root_id, &f.db),
        Err(EngineError::InvalidEntryPath(_))
    ));
    assert_eq!(f.snapshot()?, before);
    Ok(())
}

#[test]
fn synchronization_rejects_uninitialized_root() -> Result<(), Box<dyn Error>> {
    let f = Fixture::new()?;
    assert!(
        matches!(sync_root(f.root_id, &f.db), Err(EngineError::NotInitializedRoot(id)) if id == f.root_id)
    );
    assert!(
        matches!(sync_path(f.root_id, &f.db, Path::new("file")), Err(EngineError::NotInitializedRoot(id)) if id == f.root_id)
    );
    Ok(())
}
