use std::path::Path;

use super::portable_path;
use crate::{
    error::EngineError,
    snapshot::{sync_path, sync_root},
};

mod boundaries;
mod diff;
mod initial_snapshot;
mod sync;

fn sync_root_for_test(root_id: i64, db_path: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    sync_root(&mut conn, &root)
}

fn sync_path_for_test(root_id: i64, db_path: &Path, target: &Path) -> Result<(), EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;
    sync_path(&mut conn, &root, target)
}

#[test]
fn rejects_invalid_entry_paths() {
    for path in ["", "..", "invalid\\name"] {
        assert!(
            matches!(portable_path(Path::new(path)), Err(EngineError::InvalidEntryPath(p)) if p == Path::new(path))
        );
    }
}
