use std::{path::Path, sync::mpsc};

use rfsync_fs::FsEvent;

use crate::{error::EngineError, sync_root};

pub fn watch_root(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    let root_path = Path::new(&root.local_path);

    let (tx, rx) = mpsc::channel();
    let _watcher = rfsync_fs::watch(root_path, tx)?;

    for result in rx {
        println!("event: {result:?}");
        match result? {
            FsEvent::Changed(_) | FsEvent::Rescan => {
                sync_root(root_id, db_path)?;
            }
        }
    }

    Ok(())
}
