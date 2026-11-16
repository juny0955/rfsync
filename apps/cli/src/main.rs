use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rfsync", version, about = "파일 동기화 도구")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 동기화 폴더를 등록합니다.
    Add {
        /// 동기화할 폴더 경로
        path: PathBuf,
    },
    /// 등록된 폴더를 감시하고 동기화합니다.
    Watch {
        /// 감시할 폴더 경로
        path: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let db_path = PathBuf::from("./example.db");

    match cli.command {
        Command::Add { path } => {
            let root_id = match rfsync_engine::register_root(&path, &db_path) {
                Ok(id) => id,
                Err(error) => {
                    eprintln!("오류: {error}");
                    return ExitCode::FAILURE;
                }
            };
            println!("root 등록 완료: {}", path.display());

            if let Err(error) = rfsync_engine::create_initial_snapshot(root_id, &db_path) {
                eprintln!("오류: {error}");
                return ExitCode::FAILURE;
            }
            println!("초기 스냅샷 생성 완료");
        }
        Command::Watch { path } => {
            if let Err(e) = rfsync_engine::watch_path(&path, &db_path) {
                eprintln!("동기화 실패: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}
