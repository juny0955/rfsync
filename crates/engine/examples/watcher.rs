use std::{error::Error, path::Path};

use rfsync_engine::{load_or_create_identity, watch_root};

fn main() -> Result<(), Box<dyn Error>> {
    let root_id = 1;
    let db_path = Path::new("./example.db");

    let identity = load_or_create_identity()?;
    watch_root(root_id, db_path, identity.device_id())?;

    Ok(())
}
