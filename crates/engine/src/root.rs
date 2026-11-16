use std::path::Path;

use crate::error::EngineError;

pub fn register_root(root: &Path, db_path: &Path) -> Result<i64, EngineError> {
    let conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root
        .to_str()
        .ok_or_else(|| EngineError::InvalidRootPath(absolute_root.clone()))?;

    if rfsync_storage::exists_root_path(&conn, local_path)? {
        return Err(EngineError::RootAlreadyExists(absolute_root));
    }

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;
    Ok(root_id)
}
