use std::{error::Error, io, path::Path};

pub fn register_root(root: &Path, db_path: &Path) -> Result<i64, Box<dyn Error>> {
    let conn = rfsync_storage::open(db_path)?;

    let absolute_root = root.canonicalize()?;
    let local_path = absolute_root.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "root를 문자열로 표현할 수 없습니다",
        )
    })?;

    let root_id = rfsync_storage::insert_root(&conn, local_path)?;
    Ok(root_id)
}
