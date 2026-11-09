use std::{error::Error, io, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "잘못된 명령입니다"))?;

    let result = rfsync_fs::scan(&root)?;
    println!("{result:#?}");

    Ok(())
}
