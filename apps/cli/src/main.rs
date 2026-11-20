use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use rfsync_engine::DEFAULT_PORT;

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
    /// 페어링을 시작합니다.
    PairListen {
        /// 로컬 TCP·UDP 포트
        #[arg(long, default_value_t = DEFAULT_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// 주소와 코드를 입력하여 페어링합니다.
    PairConnect {
        /// peer 주소
        addr: SocketAddr,
        /// 페어링 코드
        code: String,
        /// 로컬 UDP 포트
        #[arg(long, default_value_t = DEFAULT_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let db_path = PathBuf::from("./example.db");

    match cli.command {
        Command::Add { path } => {
            if let Err(error) = rfsync_engine::register_root(&path, &db_path) {
                eprintln!("오류: {error}");
                return ExitCode::FAILURE;
            }
            println!("root 등록 및 초기 스냅샷 생성 완료");
        }
        Command::Watch { path } => {
            if let Err(e) = rfsync_engine::watch_path(&path, &db_path) {
                eprintln!("동기화 실패: {e}");
                return ExitCode::FAILURE;
            }
        }
        Command::PairListen { port } => {
            if let Err(error) = rfsync_engine::listen(port, &db_path).await {
                eprintln!("페어링 실패: {error}");
                return ExitCode::FAILURE;
            }
            println!("페어링 성공");
        }
        Command::PairConnect { addr, code, port } => {
            if let Err(error) = rfsync_engine::connect(port, addr, &code, &db_path).await {
                eprintln!("페어링 실패: {error}");
                return ExitCode::FAILURE;
            }
            println!("페어링 성공");
        }
    }

    ExitCode::SUCCESS
}
