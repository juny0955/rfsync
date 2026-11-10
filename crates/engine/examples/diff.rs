use std::{error::Error, path::Path};

use rfsync_engine::diff::diff_snapshot;

fn main() -> Result<(), Box<dyn Error>> {
    let result = diff_snapshot(1, Path::new("./example.db"))?;

    println!("result: {:#?}", result);

    Ok(())
}
