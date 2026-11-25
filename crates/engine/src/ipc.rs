//! CLI의 요청을 받는 로컬 소켓. 같은 기기에서 같은 사용자만 접속할 수 있다(권한 0600).

use std::{
    fs, io,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixStream as StdUnixStream,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rfsync_ipc::{IpcError, Request, Response, read_frame_async, write_frame_async};
use tokio::{
    io::AsyncReadExt,
    net::{UnixListener, UnixStream},
    sync::mpsc::Sender,
    task::JoinHandle,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::{
    error::EngineError,
    peer::{pair_connect, pair_listen},
};

/// 연결을 받지 못했을 때 다시 시도하기 전에 쉬는 시간. 파일 디스크립터가 모자란 경우 등에 바쁜 반복을 막는다.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// 요청을 처리하는 데 필요한 데몬의 상태와, 처리한 결과를 데몬 본체에 알리는 송신구.
pub(crate) struct IpcContext {
    pub db_path: PathBuf,
    /// 페어링 접속을 기다리는 TCP 포트. 데몬의 QUIC 포트와 같다.
    pub port: u16,
    /// 내 인증서.
    pub cert: Vec<u8>,
    /// 루트를 등록했다는 알림. 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽으므로 가득 차도 괜찮다.
    pub root_added: Sender<()>,
    /// 장치를 페어링했다는 알림.
    pub devices_changed: Sender<()>,
}

/// 소켓을 열고 요청을 처리하는 작업. 버려지면 작업을 멈추고 소켓 파일을 지운다.
pub(crate) struct IpcServer {
    path: PathBuf,
    task: JoinHandle<()>,
}

/// `path`에 소켓을 열고 요청을 받기 시작한다. tokio 런타임 안에서 불러야 한다.
///
/// 이미 같은 경로에 다른 데몬이 떠 있으면 실패한다. 비정상 종료로 남은 소켓 파일은 지우고 다시 연다.
pub(crate) fn start(path: &Path, context: IpcContext) -> Result<IpcServer, EngineError> {
    let listener = bind(path)?;
    info!("CLI 요청을 받습니다: {}", path.display());
    Ok(IpcServer {
        path: path.to_path_buf(),
        task: tokio::spawn(serve(listener, Arc::new(context))),
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

async fn serve(listener: UnixListener, context: Arc<IpcContext>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let context = context.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, &context).await {
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
async fn handle(mut stream: UnixStream, context: &IpcContext) -> Result<(), IpcError> {
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
    match tokio::task::spawn_blocking(move || crate::register_root(&path, &db_path)).await {
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
    let code = rfsync_network::generate_pairing_code();
    write_frame_async(&mut stream, &Response::PairingCode(code.clone())).await?;

    let (mut reader, mut writer) = stream.split();
    let pairing = pair_listen(context.port, &context.cert, &code, &context.db_path);
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use rfsync_network::generate_certificate;
    use tokio::sync::mpsc::{self, Receiver};

    use super::*;

    /// 알림 수신구는 버려도 서버는 동작한다. 알림을 확인할 테스트만 받아서 쓴다.
    fn context(db_path: PathBuf) -> (IpcContext, Receiver<()>, Receiver<()>) {
        let (root_added, root_added_rx) = mpsc::channel(1);
        let (devices_changed, devices_changed_rx) = mpsc::channel(1);
        let context = IpcContext {
            db_path,
            port: 0,
            cert: Vec::new(),
            root_added,
            devices_changed,
        };
        (context, root_added_rx, devices_changed_rx)
    }

    fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("rfsync.sock")
    }

    #[tokio::test]
    async fn add_root는_폴더를_루트로_등록한다() {
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "hello").unwrap();
        let path = socket_in(&dir);
        let db = dir.path().join("test.db");
        let _server = start(&path, context(db.clone()).0).unwrap();

        let response = ask(
            &path,
            &Request::AddRoot {
                path: root.path().to_path_buf(),
            },
        )
        .await;

        assert_eq!(response, Response::Ok);
        let conn = rfsync_storage::open(&db).unwrap();
        let roots = rfsync_storage::list_roots(&conn).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].has_snapshot);
    }

    #[tokio::test]
    async fn add_root는_상대_경로를_거부한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let db = dir.path().join("test.db");
        let _server = start(&path, context(db.clone()).0).unwrap();

        let response = ask(
            &path,
            &Request::AddRoot {
                path: "relative".into(),
            },
        )
        .await;

        assert!(matches!(response, Response::Error(_)));
        let conn = rfsync_storage::open(&db).unwrap();
        assert!(rfsync_storage::list_roots(&conn).unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_root는_없는_폴더면_error로_답하고_서버는_계속_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();

        let response = ask(
            &path,
            &Request::AddRoot {
                path: dir.path().join("missing"),
            },
        )
        .await;

        assert!(matches!(response, Response::Error(_)));
        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn add_root를_마치면_데몬에_알린다() {
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (ctx, mut root_added, _) = context(dir.path().join("test.db"));
        let _server = start(&path, ctx).unwrap();

        ask(
            &path,
            &Request::AddRoot {
                path: root.path().to_path_buf(),
            },
        )
        .await;

        assert!(root_added.try_recv().is_ok());
    }

    /// 포트 하나를 골라 상대가 페어링 접속을 기다리는 쪽(listen)과 거는 쪽(connect)의 서버를 만든다.
    fn pair_servers(
        dir: &tempfile::TempDir,
    ) -> (PathBuf, PathBuf, u16, [Receiver<()>; 2], [IpcServer; 2]) {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let make = |name: &str, port: u16| {
            let (cert, _) = generate_certificate("rfsync").unwrap();
            let socket = dir.path().join(format!("{name}.sock"));
            let (mut ctx, _, devices) = context(dir.path().join(format!("{name}.db")));
            ctx.port = port;
            ctx.cert = cert;
            let server = start(&socket, ctx).unwrap();
            (socket, devices, server)
        };
        let (listen_socket, listen_devices, listen_server) = make("listen", port);
        let (connect_socket, connect_devices, connect_server) = make("connect", 0);
        (
            listen_socket,
            connect_socket,
            port,
            [listen_devices, connect_devices],
            [listen_server, connect_server],
        )
    }

    #[tokio::test]
    async fn 페어링하면_양쪽이_서로를_저장하고_데몬에_알린다() {
        let dir = tempfile::tempdir().unwrap();
        let (
            listen_socket,
            connect_socket,
            port,
            [mut listen_devices, mut connect_devices],
            _servers,
        ) = pair_servers(&dir);

        let mut listening = UnixStream::connect(&listen_socket).await.unwrap();
        write_frame_async(&mut listening, &Request::PairListen)
            .await
            .unwrap();
        let Response::PairingCode(code) = read_frame_async(&mut listening).await.unwrap() else {
            panic!("코드가 먼저 와야 합니다");
        };
        // 코드를 받았으면 접속을 기다리는 중이다. 잠깐 두어 TCP를 열 시간을 준다.
        sleep(Duration::from_millis(100)).await;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let connected = ask(&connect_socket, &Request::PairConnect { addr, code }).await;

        assert_eq!(connected, Response::Ok);
        assert_eq!(
            read_frame_async::<Response>(&mut listening).await.unwrap(),
            Response::Ok
        );
        let listen_db = rfsync_storage::open(&dir.path().join("listen.db")).unwrap();
        let connect_db = rfsync_storage::open(&dir.path().join("connect.db")).unwrap();
        let on_listen = rfsync_storage::list_devices(&listen_db).unwrap();
        let on_connect = rfsync_storage::list_devices(&connect_db).unwrap();
        assert_eq!(on_listen.len(), 1);
        assert_eq!(on_listen[0].endpoint, None);
        assert_eq!(on_connect.len(), 1);
        assert_eq!(
            on_connect[0].endpoint.as_deref(),
            Some(addr.to_string().as_str())
        );
        assert!(listen_devices.try_recv().is_ok());
        assert!(connect_devices.try_recv().is_ok());
    }

    #[tokio::test]
    async fn 코드가_틀리면_error로_답하고_장치를_저장하지_않는다() {
        let dir = tempfile::tempdir().unwrap();
        let (listen_socket, connect_socket, port, [_, mut connect_devices], _servers) =
            pair_servers(&dir);
        let mut listening = UnixStream::connect(&listen_socket).await.unwrap();
        write_frame_async(&mut listening, &Request::PairListen)
            .await
            .unwrap();
        let _code: Response = read_frame_async(&mut listening).await.unwrap();
        sleep(Duration::from_millis(100)).await;

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let connected = ask(
            &connect_socket,
            &Request::PairConnect {
                addr,
                code: "0-wrong-code".into(),
            },
        )
        .await;

        assert!(matches!(connected, Response::Error(_)));
        let connect_db = rfsync_storage::open(&dir.path().join("connect.db")).unwrap();
        assert!(
            rfsync_storage::list_devices(&connect_db)
                .unwrap()
                .is_empty()
        );
        assert!(connect_devices.try_recv().is_err());
    }

    #[tokio::test]
    async fn 상대가_없어_접속하지_못하면_error로_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let (mut ctx, _, _) = context(dir.path().join("test.db"));
        ctx.cert = generate_certificate("rfsync").unwrap().0;
        let _server = start(&path, ctx).unwrap();
        let closed_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = SocketAddr::from(([127, 0, 0, 1], closed_port));

        let response = ask(
            &path,
            &Request::PairConnect {
                addr,
                code: "1-abc".into(),
            },
        )
        .await;

        assert!(matches!(response, Response::Error(_)));
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
        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();

        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 소켓_파일은_같은_사용자만_접속하도록_0600이다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn 연결을_여러_개_동시에_받는다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();

        let (a, b) = tokio::join!(ask(&path, &Request::Ping), ask(&path, &Request::Ping));

        assert_eq!((a, b), (Response::Ok, Response::Ok));
    }

    #[tokio::test]
    async fn 읽을_수_없는_요청을_받아도_서버는_계속_답한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();
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
        let server = start(&path, context(dir.path().join("test.db")).0).unwrap();
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

        let _server = start(&path, context(dir.path().join("test.db")).0).unwrap();

        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 이미_데몬이_떠_있으면_실패하고_기존_소켓은_그대로_둔다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        let _first = start(&path, context(dir.path().join("test.db")).0).unwrap();

        let second = start(&path, context(dir.path().join("test.db")).0);

        assert!(matches!(second, Err(EngineError::DaemonAlreadyRunning(_))));
        assert_eq!(ask(&path, &Request::Ping).await, Response::Ok);
    }

    #[tokio::test]
    async fn 소켓이_아닌_파일은_지우지_않고_실패한다() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_in(&dir);
        fs::write(&path, b"keep").unwrap();

        let result = start(&path, context(dir.path().join("test.db")).0);

        assert!(matches!(result, Err(EngineError::NotASocket(_))));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }
}
