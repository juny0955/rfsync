use std::{path::Path, sync::mpsc};

use rfsync_fs::FsEvent;

use crate::{error::EngineError, sync_path, sync_root};

pub fn watch_root(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    let root_path = Path::new(&root.local_path);

    let (tx, rx) = mpsc::channel();
    let _watcher = rfsync_fs::watch(root_path, tx)?;

    for result in rx {
        match result? {
            FsEvent::Changed(path) => {
                let relative_path = match path.strip_prefix(root_path) {
                    Ok(relative) if !relative.as_os_str().is_empty() => relative,
                    _ => {
                        if let Err(e) = sync_root(root_id, db_path) {
                            eprintln!("동기화 오류: {e}");
                        }
                        continue;
                    }
                };

                if let Err(e) = sync_path(root_id, db_path, relative_path) {
                    eprintln!("동기화 오류: {e}");
                }
            }
            FsEvent::Rescan => {
                if let Err(e) = sync_root(root_id, db_path) {
                    eprintln!("동기화 오류: {e}");
                }
            }
        }
    }

    Ok(())
}
