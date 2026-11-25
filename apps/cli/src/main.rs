use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use rfsync_engine::{DEFAULT_PORT, db_path, socket_path};
use rfsync_ipc::{Request, Response};

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
    // 로그는 프로그램의 출력(stdout)과 섞이지 않도록 stderr로 보낸다.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let db_path = db_path();

    match cli.command {
        Command::Add { path } => {
            // 데몬과 현재 폴더가 다를 수 있으므로 절대 경로로 바꿔 보낸다.
            let path = match path.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("오류: {}: {error}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            match rfsync_ipc::request(&socket_path(db_path), &Request::AddRoot { path }) {
                Ok(Response::Ok) => println!("root 등록 및 초기 스냅샷 생성 완료"),
                Ok(Response::Error(message)) => {
                    eprintln!("오류: {message}");
                    return ExitCode::FAILURE;
                }
                Ok(other) => {
                    eprintln!("오류: 예상하지 못한 응답입니다: {other:?}");
                    return ExitCode::FAILURE;
                }
                Err(error) => {
                    eprintln!(
                        "오류: 데몬에 연결하지 못했습니다. 데몬이 실행 중인지 확인하세요: {error}"
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::PairListen { port } => {
            if let Err(error) =
                rfsync_engine::listen(port, db_path, |code| println!("code: {code}")).await
            {
                eprintln!("페어링 실패: {error}");
                return ExitCode::FAILURE;
            }
            println!("페어링 성공");
        }
        Command::PairConnect { addr, code, port } => {
            if let Err(error) = rfsync_engine::connect(port, addr, &code, db_path).await {
                eprintln!("페어링 실패: {error}");
                return ExitCode::FAILURE;
            }
            println!("페어링 성공");
        }
    }

    ExitCode::SUCCESS
}
