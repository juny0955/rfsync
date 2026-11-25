use std::path::PathBuf;

use rfsync_ipc::{IpcError, Request, Response, read_frame_async, write_frame_async};
use tokio::{io::AsyncReadExt, net::UnixStream};
use tracing::debug;

use super::IpcContext;
use crate::peer::{bind_pair_listener, pair_connect, pair_listen};

/// 연결 하나에서 요청 하나를 읽고 응답한다.
pub(super) async fn handle(mut stream: UnixStream, context: &IpcContext) -> Result<(), IpcError> {
    let request: Request = read_frame_async(&mut stream).await?;
    let response = match request {
        Request::Ping => Response::Ok,
        Request::AddRoot { path } => add_root(path, context).await,
        Request::PairListen => return pair_listen_request(stream, context).await,
        Request::PairConnect { addr, code } => {
            match pair_connect(addr, &context.cert, &code, &context.db_path).await {
                Ok(()) => paired(context),
                Err(e) => Response::Error(e.to_string()),
            }
        }
    };
    write_frame_async(&mut stream, &response).await
}

/// 폴더를 루트로 등록한다. 초기 스냅샷이 디스크를 훑으므로 블로킹 스레드에서 돈다.
async fn add_root(path: PathBuf, context: &IpcContext) -> Response {
    if !path.is_absolute() {
        return Response::Error("경로는 절대 경로여야 합니다".into());
    }
    let db_path = context.db_path.clone();
    let me = crate::DeviceId::from_cert(&context.cert);
    match tokio::task::spawn_blocking(move || crate::register_root(&path, &db_path, me)).await {
        Ok(Ok(_)) => {
            // 가득 차 있어도 받는 쪽이 곧 읽을 것이므로 무시한다.
            let _ = context.root_added.try_send(());
            Response::Ok
        }
        Ok(Err(e)) => Response::Error(e.to_string()),
        Err(e) => Response::Error(format!("등록 작업이 중단되었습니다: {e}")),
    }
}

/// 페어링을 마쳤음을 데몬 본체에 알려 새 장치를 받아들이게 한다.
fn paired(context: &IpcContext) -> Response {
    let _ = context.devices_changed.try_send(());
    Response::Ok
}

/// 코드를 먼저 보내고, 상대가 접속해 페어링을 마치면 결과를 보낸다.
/// 기다리는 동안 CLI가 연결을 끊으면(예: Ctrl-C) 기다리기를 그만둔다.
async fn pair_listen_request(mut stream: UnixStream, context: &IpcContext) -> Result<(), IpcError> {
    // 코드를 알리기 전에 접속 받을 자리부터 잡는다. 상대가 코드를 받자마자 접속해도 놓치지 않는다.
    let listener = match bind_pair_listener(context.port).await {
        Ok(listener) => listener,
        Err(e) => return write_frame_async(&mut stream, &Response::Error(e.to_string())).await,
    };
    let code = rfsync_network::generate_pairing_code();
    write_frame_async(&mut stream, &Response::PairingCode(code.clone())).await?;

    let (mut reader, mut writer) = stream.split();
    let pairing = pair_listen(listener, &context.cert, &code, &context.db_path);
    let mut hangup = [0u8; 1];
    let response = tokio::select! {
        result = pairing => match result {
            Ok(()) => paired(context),
            Err(e) => Response::Error(e.to_string()),
        },
        // CLI는 코드를 받은 뒤 아무것도 보내지 않는다. 읽기가 끝났다면 연결이 끊긴 것이다.
        _ = reader.read(&mut hangup) => {
            debug!("CLI가 연결을 끊어 페어링 대기를 그만둡니다");
            return Ok(());
        }
    };
    write_frame_async(&mut writer, &response).await
}
