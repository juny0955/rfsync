use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use rfsync_engine::{db_path, socket_path};
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
    /// 페어링을 시작합니다. 코드를 출력하고 상대가 접속할 때까지 기다립니다.
    PairListen,
    /// 주소와 코드를 입력하여 페어링합니다.
    PairConnect {
        /// peer 주소
        addr: SocketAddr,
        /// 페어링 코드
        code: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let socket = socket_path(db_path());

    let request = match cli.command {
        Command::Add { path } => {
            // 데몬과 현재 폴더가 다를 수 있으므로 절대 경로로 바꿔 보낸다.
            match path.canonicalize() {
                Ok(path) => Request::AddRoot { path },
                Err(error) => {
                    eprintln!("오류: {}: {error}", path.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::PairListen => Request::PairListen,
        Command::PairConnect { addr, code } => Request::PairConnect { addr, code },
    };
    let success = match &request {
        Request::AddRoot { .. } => "root 등록 및 초기 스냅샷 생성 완료",
        _ => "페어링 성공",
    };

    let result = rfsync_ipc::request_streaming(&socket, &request, |response| {
        if let Response::PairingCode(code) = response {
            println!("code: {code}");
        }
    });
    match result {
        Ok(Response::Ok) => println!("{success}"),
        Ok(Response::Error(message)) => {
            eprintln!("오류: {message}");
            return ExitCode::FAILURE;
        }
        Ok(other) => {
            eprintln!("오류: 예상하지 못한 응답입니다: {other:?}");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("오류: 데몬에 연결하지 못했습니다. 데몬이 실행 중인지 확인하세요: {error}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
