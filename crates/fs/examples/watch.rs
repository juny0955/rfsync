use std::{error::Error, path::Path, sync::mpsc};

use rfsync_fs::watch;

fn main() -> Result<(), Box<dyn Error>> {
    // watcher가 보낸 이벤트를 rx에서 받습니다.
    let (tx, rx) = mpsc::channel();

    let _watcher = watch(Path::new("./crates"), tx)?;
    println!("./crates 감시 중... 종료하려면 Ctrl+C");

    for result in rx {
        match result {
            Ok(event) => println!("변경: {event:?}"),
            Err(error) => eprintln!("감시 오류: {error}"),
        }
    }

    Ok(())
}
