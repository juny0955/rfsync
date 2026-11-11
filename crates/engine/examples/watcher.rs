use std::{error::Error, path::Path};

use rfsync_engine::watch_root;

fn main() -> Result<(), Box<dyn Error>> {
    let root_id = 1;
    let db_path = Path::new("./example.db");

    watch_root(root_id, db_path)?;

    Ok(())
}
