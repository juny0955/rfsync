use std::path::{Path, PathBuf};

use rfsync_protocol::DeviceId;

use super::snapshot::create_initial_snapshot;
use crate::error::EngineError;

/// 폴더를 루트로 등록하고 초기 스냅샷을 만든다. 스냅샷의 파일은 내가 처음 본 것이므로 `me`의 카운터가 1이다.
pub fn register_root(root: &Path, db_path: &Path, me: DeviceId) -> Result<i64, EngineError> {
    let mut conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root
        .to_str()
        .ok_or_else(|| EngineError::InvalidRootPath(absolute_root.clone()))?;

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;

    create_initial_snapshot(&mut conn, root_id, me)?;
    Ok(root_id)
}

/// DB에 등록된 첫 번째 루트의 폴더 경로. 루트가 없으면 `None`이다.
///
/// 임시: 루트가 하나뿐이라고 가정해 첫 루트만 쓴다. `load_index`와 같은 가정이다.
pub(crate) fn first_root_path(db_path: &Path) -> Result<Option<PathBuf>, EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    let roots = rfsync_storage::list_roots(&conn)?;
    Ok(roots.first().map(|root| PathBuf::from(&root.local_path)))
}
