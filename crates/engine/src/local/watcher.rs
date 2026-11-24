use std::{path::Path, sync::mpsc};

use rfsync_fs::FsEvent;
use rfsync_storage::{DbConnection, Root};
use tracing::error;

use super::snapshot::{sync_path, sync_root};
use crate::error::EngineError;

/// CLI 전용
pub fn watch_path(local_path: &Path, db_path: &Path) -> Result<(), EngineError> {
    let absolute_root = local_path.canonicalize()?;
    let root_path = absolute_root
        .to_str()
        .ok_or_else(|| EngineError::InvalidRootPath(local_path.to_path_buf()))?;

    let conn = rfsync_storage::open(db_path)?;
    let root = rfsync_storage::get_root_by_path(&conn, root_path)?
        .ok_or(EngineError::RootNotFoundByPath(local_path.to_path_buf()))?;

    watch(conn, root)
}

pub fn watch_root(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    watch(conn, root)
}

fn watch(mut conn: DbConnection, root: Root) -> Result<(), EngineError> {
    if !root.has_snapshot {
        return Err(EngineError::NotInitializedRoot(root.id));
    }

    let (tx, rx) = mpsc::channel();
    let root_path = Path::new(&root.local_path);
    let _watcher = rfsync_fs::watch(root_path, tx)?;

    // 감시 이벤트 수신 전 전체 동기화
    sync_root(&mut conn, &root)?;

    for result in rx {
        match result? {
            FsEvent::Changed(path) => {
                let relative_path = match path.strip_prefix(root_path) {
                    Ok(relative) if !relative.as_os_str().is_empty() => relative,
                    _ => {
                        if let Err(e) = sync_root(&mut conn, &root) {
                            error!("동기화 오류: {e}");
                        }
                        continue;
                    }
                };

                if let Err(e) = sync_path(&mut conn, &root, relative_path) {
                    error!("동기화 오류: {e}");
                }
            }
            FsEvent::Rescan => {
                if let Err(e) = sync_root(&mut conn, &root) {
                    error!("동기화 오류: {e}");
                }
            }
        }
    }

    Ok(())
}
