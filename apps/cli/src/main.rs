use std::{path::PathBuf, process::ExitCode};

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "rfsync",
    version,
    about = "동기화할 폴더를 등록하고 초기 스냅샷을 생성합니다"
)]
struct Cli {
    /// 동기화할 폴더 경로
    root: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let db_path = PathBuf::from("./example.db");

    let root_id = match rfsync_engine::register_root(&cli.root, &db_path) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("오류: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("root 등록 완료: {}", cli.root.display());

    if let Err(error) = rfsync_engine::create_initial_snapshot(root_id, &db_path) {
        eprintln!("오류: {error}");
        return ExitCode::FAILURE;
    }
    println!("초기 스냅샷 생성 완료");
    ExitCode::SUCCESS
}
