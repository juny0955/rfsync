use std::{
    error::Error,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use rfsync_network::{Endpoint, QuicConnection, generate_certificate, open_stream};
use rfsync_protocol::VersionVector;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, watch},
    time::{sleep, timeout},
};

use super::{
    handshake::{PeerHandshake, exchange},
    *,
};
use crate::peer::{FileOffer, PeerHandle, identity::SERVER_NAME, load_index};

const TIMEOUT: Duration = Duration::from_secs(10);

/// 비어 있는 UDP 포트를 하나 고른다.
///
/// "포트 0에 바인딩해 알아낸 뒤 놓아 주는" 방식은 병렬로 도는 다른 테스트가 그 포트를 가져가는 일이 있어
/// 가끔 `Address already in use`로 실패했다. 임시 포트 범위 밖의 번호를 프로세스 안에서 하나씩 올려가며
/// 고르므로, 같은 프로세스의 테스트끼리는 같은 포트를 받지 않는다.
fn free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(20_000);
    loop {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        if UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok() {
            return port;
        }
    }
}

fn new_identity() -> Result<Identity, Box<dyn Error>> {
    let (cert, key) = generate_certificate(SERVER_NAME)?;
    Ok(Identity { cert, key })
}

/// 데몬 역할의 `run`과 접속하는 상대 피어를 한 곳에서 준비한다.
struct Fixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
    daemon: Identity,
    peer: Identity,
    port: u16,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        logs(); // 어떤 세션 테스트보다 먼저 로그 구독자를 설치한다
        let dir = tempfile::tempdir()?;
        let port = free_port();
        Ok(Self {
            db_path: dir.path().join("session.db"),
            _dir: dir,
            daemon: new_identity()?,
            peer: new_identity()?,
            port,
        })
    }

    /// 상대 피어를 페어링된 기기로 DB에 저장한다.
    fn pair_peer(&self) -> Result<(), Box<dyn Error>> {
        let conn = rfsync_storage::open(&self.db_path)?;
        rfsync_storage::upsert_device(&conn, &self.peer.cert, None, 100)?;
        Ok(())
    }

    async fn run(&self, shutdown: impl Future<Output = ()>) -> Result<(), EngineError> {
        let (events, _rx) = mpsc::channel(8);
        self.run_with(events, shutdown).await
    }

    /// 데몬의 세션을 주어진 이벤트 송신구로 실행한다. 이벤트를 받아 줄 쪽은 테스트가 맡는다.
    async fn run_with(
        &self,
        events: mpsc::Sender<PeerEvent>,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), EngineError> {
        let (_devices, devices_rx) = mpsc::channel(1);
        self.run_full(events, devices_rx, shutdown).await
    }

    /// `run_with`에 더해, 페어링된 장치가 바뀌었다는 알림을 받는 수신구도 주어진 것을 쓴다.
    async fn run_full(
        &self,
        events: mpsc::Sender<PeerEvent>,
        devices_rx: mpsc::Receiver<()>,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), EngineError> {
        let index = load_index(&self.db_path)?;
        let (_index, index_rx) = watch::channel(Arc::new(index));
        run(
            &self.daemon,
            self.port,
            &self.db_path,
            index_rx,
            events,
            devices_rx,
            shutdown,
        )
        .await
    }

    /// 주어진 신원으로 데몬에 접속한다. 엔드포인트도 연결이 살아 있는 동안 유지해야 한다.
    async fn connect(
        &self,
        identity: &Identity,
    ) -> Result<(Endpoint, QuicConnection), NetworkError> {
        let endpoint = rfsync_network::bind_quic(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rfsync_network::server_config(&identity.cert, &identity.key, &[&self.daemon.cert])?,
        )?;
        let config =
            rfsync_network::client_config(&identity.cert, &identity.key, &self.daemon.cert)?;
        let conn = rfsync_network::connect_quic(
            &endpoint,
            SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            SERVER_NAME,
            config,
        )
        .await?;
        Ok((endpoint, conn))
    }

    /// 상대 피어를 `addr`로 접속할 수 있는 기기로 DB에 저장한다.
    fn pair_peer_at(&self, addr: SocketAddr) -> Result<(), Box<dyn Error>> {
        let conn = rfsync_storage::open(&self.db_path)?;
        rfsync_storage::upsert_device(&conn, &self.peer.cert, Some(&addr.to_string()), 100)?;
        Ok(())
    }

    /// 데몬이 접속해 올 서버 역할의 상대 피어를 띄운다. 신뢰하는 쪽은 데몬뿐이다.
    fn bind_peer(&self, identity: &Identity) -> Result<Endpoint, NetworkError> {
        rfsync_network::bind_quic(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            rfsync_network::server_config(&identity.cert, &identity.key, &[&self.daemon.cert])?,
        )
    }

    /// 데몬을 띄우고 `scenario`가 끝나면 종료시킨 뒤 양쪽 결과를 확인한다.
    async fn with_daemon<F, Fut>(&self, scenario: F) -> Result<(), Box<dyn Error>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), Box<dyn Error>>>,
    {
        let (events, _rx) = mpsc::channel(8);
        self.with_daemon_events(events, scenario).await
    }

    /// `with_daemon`과 같지만 세션이 보내는 이벤트를 주어진 송신구로 받는다.
    async fn with_daemon_events<F, Fut>(
        &self,
        events: mpsc::Sender<PeerEvent>,
        scenario: F,
    ) -> Result<(), Box<dyn Error>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), Box<dyn Error>>>,
    {
        let (stop, stopped) = oneshot::channel::<()>();
        let scenario = async {
            let result = scenario().await;
            let _ = stop.send(());
            result
        };
        let (daemon, scenario) = timeout(TIMEOUT, async {
            tokio::join!(
                self.run_with(events, async {
                    let _ = stopped.await;
                }),
                scenario
            )
        })
        .await?;

        daemon?;
        scenario
    }
}

/// 상대 피어가 데몬의 접속을 기다렸다가 받는다.
async fn accept_from_daemon(peer: &Endpoint) -> Result<QuicConnection, Box<dyn Error>> {
    Ok(timeout(TIMEOUT, rfsync_network::accept_quic(peer)).await??)
}

fn hello() -> Message {
    Message::Hello {
        version: PROTOCOL_VERSION,
    }
}

fn entry(path: &str) -> Entry {
    Entry {
        path: path.to_owned(),
        size: 1,
        hash: [9; 32],
        modified_secs: 1,
        modified_nanos: 2,
        deleted: false,
        version: VersionVector::default(),
    }
}

fn index_message(entries: Vec<Entry>) -> Message {
    Message::Index { entries }
}

/// 상대 쪽에서 데몬이 보낸 첫 두 프레임(`Hello`, 파일 목록)을 그대로 읽는다.
async fn read_handshake(recv: &mut QuicRecvStream) -> Result<(Message, Message), Box<dyn Error>> {
    let first = timeout(TIMEOUT, read_frame(recv)).await??;
    let second = timeout(TIMEOUT, read_frame(recv)).await??;
    Ok((first, second))
}

/// 상대 쪽에서 자기 `Hello`와 빈 파일 목록을 보낸다. 먼저 써야 받는 쪽에 스트림이 보인다.
async fn write_handshake(send: &mut QuicSendStream) -> Result<(), Box<dyn Error>> {
    write_frame(send, &hello()).await?;
    write_frame(send, &index_message(Vec::new())).await?;
    Ok(())
}

/// 테스트에서 `tracing` 로그를 모아 보는 버퍼.
#[derive(Clone, Default)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static LOGS: std::sync::OnceLock<LogBuffer> = std::sync::OnceLock::new();

/// 프로세스 전체가 하나의 로그 버퍼를 쓴다. 모든 세션 테스트가 `Fixture::new`에서 이 함수를 먼저 부르므로
/// 어떤 로그 지점도 구독자 없이 처음 실행되지 않는다. (테스트마다 현재 스레드에만 구독자를 걸면, 다른 테스트가
/// 병렬로 같은 로그 지점을 실행하는 순간과 겹쳐 INFO 로그를 놓치는 일이 있었다.)
/// 버퍼는 테스트들이 공유하므로 각 테스트는 자기만의 문구로 찾는다.
fn logs() -> &'static LogBuffer {
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("테스트 프로세스에서 전역 구독자는 한 번만 설치한다");
        logs
    })
}

/// 상대 피어가 데몬에 접속해 핸드셰이크를 마친 모습. 연결을 쥐고 있는 동안 데몬 쪽 세션이 살아 있다.
struct HandshakenPeer {
    _endpoint: Endpoint,
    conn: QuicConnection,
    /// 데몬이 `IndexReceived`로 넘겨 준, 이 연결에 파일을 요청하는 손잡이.
    handle: PeerHandle,
    /// 핸드셰이크에 쓴 제어 스트림. 이후의 `IndexUpdate`도 이 스트림으로 오간다.
    control: (QuicSendStream, QuicRecvStream),
}

/// 상대 피어로 데몬에 접속해 핸드셰이크를 마치고, 데몬이 보낸 `IndexReceived`에서 손잡이를 꺼낸다.
async fn handshake_with_daemon(
    fixture: &Fixture,
    received: &mut mpsc::Receiver<PeerEvent>,
) -> Result<HandshakenPeer, Box<dyn Error>> {
    let (endpoint, conn) = fixture.connect(&fixture.peer).await?;
    let (mut send, mut recv) = open_stream(&conn).await?;
    write_handshake(&mut send).await?;
    read_handshake(&mut recv).await?;

    let event = timeout(TIMEOUT, received.recv())
        .await?
        .ok_or("이벤트 통로가 닫혔다")?;
    let PeerEvent::IndexReceived { peer: handle, .. } = event else {
        return Err("IndexReceived가 아니다".into());
    };
    Ok(HandshakenPeer {
        _endpoint: endpoint,
        conn,
        handle,
        control: (send, recv),
    })
}

/// 상대 피어 쪽에서 데몬이 연 스트림을 받아 첫 프레임(`FileRequest`)을 읽고,
/// 보낼 응답을 `respond`가 쓰게 한다.
async fn answer_request<F, Fut>(
    conn: &QuicConnection,
    expected_path: &str,
    respond: F,
) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(QuicSendStream) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    let (send, mut recv) = timeout(TIMEOUT, accept_stream(conn)).await??;
    let request = timeout(TIMEOUT, read_frame(&mut recv)).await??;
    assert_eq!(
        request,
        Message::FileRequest {
            path: expected_path.to_owned()
        }
    );
    respond(send).await
}

mod connection;
mod exchange;
mod file_request;
mod file_serve;
mod handshake;
mod index_update;
