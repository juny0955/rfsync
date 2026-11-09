use std::{error::Error, io, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let db_path = PathBuf::from("./example.db");

    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "잘못된 명령입니다"))?;

    let root_id = rfsync_core::register_root(&root, &db_path)?;
    rfsync_core::create_initial_snapshot(root_id, &db_path)?;

    Ok(())
}
