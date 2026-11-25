use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use rfsync_ipc::{Request, Response, read_frame_async, write_frame_async};
use rfsync_network::generate_certificate;
use tokio::{
    net::UnixStream,
    sync::mpsc::{self, Receiver},
};

use super::{IpcContext, server::IpcServer, start};
use crate::error::EngineError;

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
    let (listen_socket, connect_socket, port, [mut listen_devices, mut connect_devices], _servers) =
        pair_servers(&dir);

    let mut listening = UnixStream::connect(&listen_socket).await.unwrap();
    write_frame_async(&mut listening, &Request::PairListen)
        .await
        .unwrap();
    let Response::PairingCode(code) = read_frame_async(&mut listening).await.unwrap() else {
        panic!("코드가 먼저 와야 합니다");
    };
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
