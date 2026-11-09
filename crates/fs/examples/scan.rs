use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let result = rfsync_fs::scan(Path::new("./crates"))?;

    println!("{:?}", result);

    Ok(())
}
