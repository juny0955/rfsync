//! CLI의 요청을 받는 로컬 소켓. 같은 기기에서 같은 사용자만 접속할 수 있다(권한 0600).

use std::{
    fs, io,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixStream as StdUnixStream,
    },
    path::{Path, PathBuf},
    time::Duration,
};

use rfsync_ipc::{IpcError, Request, Response, read_frame_async, write_frame_async};
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinHandle,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::error::EngineError;

/// 연결을 받지 못했을 때 다시 시도하기 전에 쉬는 시간. 파일 디스크립터가 모자란 경우 등에 바쁜 반복을 막는다.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// 소켓을 열고 요청을 처리하는 작업. 버려지면 작업을 멈추고 소켓 파일을 지운다.
pub(crate) struct IpcServer {
    path: PathBuf,
    task: JoinHandle<()>,
}

/// `path`에 소켓을 열고 요청을 받기 시작한다. tokio 런타임 안에서 불러야 한다.
///
/// 이미 같은 경로에 다른 데몬이 떠 있으면 실패한다. 비정상 종료로 남은 소켓 파일은 지우고 다시 연다.
pub(crate) fn start(path: &Path) -> Result<IpcServer, EngineError> {
    let listener = bind(path)?;
    info!("CLI 요청을 받습니다: {}", path.display());
    Ok(IpcServer {
        path: path.to_path_buf(),
        task: tokio::spawn(serve(listener)),
    })
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.task.abort();
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(
                "소켓 파일을 지우지 못했습니다: {}: {e}",
                self.path.display()
            );
        }
    }
}

fn bind(path: &Path) -> Result<UnixListener, EngineError> {
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            remove_stale_socket(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e.into()),
    };
    // 만든 직후 바로 같은 사용자만 접속하도록 권한을 좁힌다.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// 같은 경로에 남은 소켓 파일이 아무도 받지 않는 것이면 지운다.
/// 소켓이 아닌 파일은 지우지 않고, 누군가 받고 있으면 이미 데몬이 떠 있는 것이다.
fn remove_stale_socket(path: &Path) -> Result<(), EngineError> {
    if !fs::symlink_metadata(path)?.file_type().is_socket() {
        return Err(EngineError::NotASocket(path.to_path_buf()));
    }
    match StdUnixStream::connect(path) {
        Ok(_) => Err(EngineError::DaemonAlreadyRunning(path.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(e.into()),
    }
}

async fn serve(listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle(stream).await {
                        debug!("CLI 요청을 처리하지 못했습니다: {e}");
                    }
                });
            }
            Err(e) => {
                warn!("CLI 연결을 받지 못했습니다: {e}");
                sleep(ACCEPT_RETRY_DELAY).await;
            }
        }
    }
}

/// 연결 하나에서 요청 하나를 읽고 응답한다.
async fn handle(mut stream: UnixStream) -> Result<(), IpcError> {
    let request: Request = read_frame_async(&mut stream).await?;
    let response = match request {
        Request::Ping => Response::Ok,
        _ => Response::Error("아직 지원하지 않는 요청입니다".into()),
    };
    write_frame_async(&mut stream, &response).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("rfsync.sock")
    }

    /// 요청 하나를 보내고 응답 하나를 읽는다.
    async fn ask(path: &Path, request: &Request) -> Response {
        let mut stream = UnixStream::connect(path).await.unwrap();
        write_frame_async(&mut stream, request).await.unwrap();
        read_frame_async(&mut stream).await.unwrap()
    }

    #[tokio::test]
    async fn ping에는_ok로_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path).unwrap();

        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 아직_지원하지_않는_요청에는_error로_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path).unwrap();

        let response = ask(&path, &Request::PairListen).await;

        assert!(matches!(response, Response::Error(_)));
    }

    #[tokio::test]
    async fn 소켓_파일은_같은_사용자만_접속하도록_0600이다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn 연결을_여러_개_동시에_받는다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path).unwrap();

        let (a, b) = tokio::join!(ask(&path, &Request::Ping), ask(&path, &Request::Ping));

        assert_eq!((a, b), (Response::Ok, Response::Ok));
    }

    #[tokio::test]
    async fn 읽을_수_없는_요청을_받아도_서버는_계속_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path).unwrap();
        let mut broken = UnixStream::connect(&path).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut broken, &[0, 0, 0, 1, 0xFF])
            .await
            .unwrap();
        drop(broken);

        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 버려지면_소켓_파일을_지운다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let server = start(&path).unwrap();
        assert!(path.exists());

        drop(server);

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn 비정상_종료로_남은_소켓_파일은_지우고_다시_연다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        // 소켓 파일만 남기고 받는 쪽이 사라진 상태를 만든다.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let _server = start(&path).unwrap();

        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 이미_데몬이_떠_있으면_실패하고_기존_소켓은_그대로_둔다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _first = start(&path).unwrap();

        let second = start(&path);

        assert!(matches!(second, Err(EngineError::DaemonAlreadyRunning(_))));
        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 소켓이_아닌_파일은_지우지_않고_실패한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        fs::write(&path, b"keep").unwrap();

        let result = start(&path);

        assert!(matches!(result, Err(EngineError::NotASocket(_))));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }
}
