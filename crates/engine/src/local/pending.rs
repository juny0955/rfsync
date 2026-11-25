//! 상대에게서 받은 변경을 디스크에 적용하기 전에 남기는 기록(`sync_pending`).
//! 감시자가 그 변경을 보고 내가 고친 것으로 오해하지 않게 하는 표시다.

use std::path::Path;

use rfsync_protocol::Entry;
use rfsync_storage::SyncPending;

use crate::error::EngineError;

/// 받아 적용할 변경을 기록한다. 디스크를 바꾸기 **전에** 불러야, 감시자가 변경을 볼 때 기록이 이미 있다.
///
/// `deleted`가 참이면 상대가 지운 파일을 지우려는 것이다.
///
/// 임시: 루트가 하나뿐이라고 가정해 첫 루트에 기록한다. `first_root_path`와 같은 가정이다.
pub(crate) fn record_pending(
    db_path: &Path,
    entry: &Entry,
    deleted: bool,
) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let Some(root) = rfsync_storage::list_roots(&conn)?.into_iter().next() else {
        return Ok(());
    };
    let pending = SyncPending {
        path: entry.path.clone(),
        hash: blake3::Hash::from_bytes(entry.hash).to_hex().to_string(),
        deleted,
        version: entry.version.to_bytes()?,
    };
    rfsync_storage::put_sync_pending(&conn, root.id, &pending)?;
    Ok(())
}

/// 적용하지 못했을 때 남은 기록을 지운다. 지우지 않으면 나중에 내가 같은 파일을 고쳤을 때 잘못 쓰인다.
pub(crate) fn clear_pending(db_path: &Path, path: &str) -> Result<(), EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let Some(root) = rfsync_storage::list_roots(&conn)?.into_iter().next() else {
        return Ok(());
    };
    rfsync_storage::delete_sync_pending(&conn, root.id, path)?;
    Ok(())
}
