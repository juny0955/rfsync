use std::{path::Path, sync::mpsc};

use rfsync_fs::FsEvent;
use rfsync_protocol::DeviceId;
use rfsync_storage::{DbConnection, Root};
use tokio::sync::mpsc::{Sender, error::TrySendError};
use tracing::error;

use super::snapshot::{sync_path, sync_root};
use crate::error::EngineError;

pub fn watch_root(root_id: i64, db_path: &Path, me: DeviceId) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    watch(conn, root, me, || true)
}

/// 데몬 전용. 등록된 첫 루트를 별도 스레드에서 감시해 DB를 갱신하고, 갱신할 때마다 `changed`에 신호를 보낸다.
/// 루트가 없으면 아무것도 하지 않는다. 신호를 받을 쪽이 사라지면 감시를 멈춘다.
///
/// 임시: 루트가 하나뿐이라고 가정해 첫 루트만 감시한다. `load_index`와 같은 가정이다.
pub(crate) fn spawn_watcher(
    db_path: &Path,
    changed: Sender<()>,
    me: DeviceId,
) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let Some(root) = rfsync_storage::list_roots(&conn)?.into_iter().next() else {
        return Ok(());
    };

    std::thread::spawn(move || {
        // 신호가 이미 쌓여 있으면(Full) 받는 쪽이 곧 최신 상태를 읽으므로 또 쌓지 않는다.
        let notify = || !matches!(changed.try_send(()), Err(TrySendError::Closed(_)));
        if let Err(e) = watch(conn, root, me, notify) {
            error!("폴더 감시가 멈췄습니다: {e}");
        }
    });
    Ok(())
}

/// 루트를 감시하며 변경을 DB에 반영한다. 반영할 때마다 `on_synced`를 부르고, `false`를 돌려주면 감시를 끝낸다.
fn watch(
    mut conn: DbConnection,
    root: Root,
    me: DeviceId,
    mut on_synced: impl FnMut() -> bool,
) -> Result<(), EngineError> {
    if !root.has_snapshot {
        return Err(EngineError::NotInitializedRoot(root.id));
    }

    let (tx, rx) = mpsc::channel();
    let root_path = Path::new(&root.local_path);
    let _watcher = rfsync_fs::watch(root_path, tx)?;

    // 감시 이벤트 수신 전 전체 동기화
    sync_root(&mut conn, &root, me)?;

    for result in rx {
        match result? {
            FsEvent::Changed(path) => {
                let result = match path.strip_prefix(root_path) {
                    Ok(relative) if !relative.as_os_str().is_empty() => {
                        sync_path(&mut conn, &root, relative, me)
                    }
                    _ => sync_root(&mut conn, &root, me),
                };
                if let Err(e) = result {
                    error!("동기화 오류: {e}");
                }
            }
            FsEvent::Rescan => {
                if let Err(e) = sync_root(&mut conn, &root, me) {
                    error!("동기화 오류: {e}");
                }
            }
        }

        if !on_synced() {
            break;
        }
    }

    Ok(())
}
