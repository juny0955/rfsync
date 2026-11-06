use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let paths = rfsync_fs::scan(Path::new("./"))?;

    for path in paths {
        println!("{:?}", path.to_str());
    }

    Ok(())
}
