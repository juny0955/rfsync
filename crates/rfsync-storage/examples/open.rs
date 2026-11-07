use std::error::Error;
use std::path::Path;

use rfsync_storage::open;

fn main() -> Result<(), Box<dyn Error>> {
    let path = Path::new("./example.db");

    let _conn = open(path)?;

    Ok(())
}
