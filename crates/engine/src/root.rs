use std::path::Path;

use crate::{error::EngineError, snapshot::create_initial_snapshot};

pub fn register_root(root: &Path, db_path: &Path) -> Result<i64, EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root
        .to_str()
        .ok_or_else(|| EngineError::InvalidRootPath(absolute_root.clone()))?;

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;

    create_initial_snapshot(&mut conn, root_id)?;
    Ok(root_id)
}
