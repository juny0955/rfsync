use std::{error::Error, path::Path};

use rfsync_engine::{diff::calc_diff, error::EngineError};

fn main() -> Result<(), Box<dyn Error>> {
    let root_id = 1;
    let db_path = Path::new("./example.db");

    let conn = rfsync_storage::open(db_path)?;
    let root =
        rfsync_storage::get_root(&conn, root_id)?.ok_or(EngineError::RootNotFound(root_id))?;

    let entries = rfsync_fs::scan(Path::new(&root.local_path))?;

    let folders = rfsync_storage::get_folders_by_root_id(&conn, root_id)?;
    let files = rfsync_storage::get_files_by_root_id(&conn, root_id)?;

    let result = calc_diff(&entries, &folders, &files)?;
    println!("result: {:#?}", result);

    Ok(())
}
