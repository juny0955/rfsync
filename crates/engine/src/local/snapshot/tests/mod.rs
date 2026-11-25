use std::path::Path;

use rfsync_protocol::DeviceId;

use super::portable_path;
use super::{create_initial_snapshot, sync_path, sync_root};
use crate::error::EngineError;

mod boundaries;
mod diff;
mod initial_snapshot;
mod sync;

/// 테스트에서 내 장치로 쓰는 고정 ID.
pub(super) fn test_device() -> DeviceId {
    DeviceId::from_cert(b"test")
}

fn create_initial_snapshot_for_test(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    create_initial_snapshot(&mut conn, root_id, crate::DeviceId::from_cert(b"test"))
}

fn sync_root_for_test(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    sync_root(&mut conn, &root, test_device())
}

fn sync_path_for_test(root_id: i64, db_path: &Path, target: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    sync_path(&mut conn, &root, target, test_device())
}

#[test]
fn rejects_invalid_entry_paths() {
    for path in ["", "..", "invalid\\name"] {
        assert!(
            matches!(portable_path(Path::new(path)), Err(EngineError::InvalidEntryPath(p)) if p == Path::new(path))
        );
    }
}
