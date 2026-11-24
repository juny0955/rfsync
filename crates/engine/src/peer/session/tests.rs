use std::{
    error::Error,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use rfsync_network::{Endpoint, QuicConnection, generate_certificate};
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};

use super::*;
use crate::peer::identity::SERVER_NAME;

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

    fn run(
        &self,
        shutdown: impl Future<Output = ()>,
    ) -> impl Future<Output = Result<(), EngineError>> {
        run(&self.daemon, self.port, &self.db_path, shutdown)
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
        let (stop, stopped) = oneshot::channel::<()>();
        let scenario = async {
            let result = scenario().await;
            let _ = stop.send(());
            result
        };
        let (daemon, scenario) = timeout(TIMEOUT, async {
            tokio::join!(
                self.run(async {
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

#[tokio::test]
async fn dials_the_stored_endpoint_of_a_paired_device() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let conn = accept_from_daemon(&peer).await?;
            // 데몬이 연결 핸들을 놓았다면 이 시간 안에 연결이 닫힌다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn dials_again_after_the_connection_is_closed() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let first = accept_from_daemon(&peer).await?;
            first.close(0u32.into(), b"test");

            // 첫 대기 시간(1초) 뒤에 데몬이 다시 접속해 와야 한다.
            let second = accept_from_daemon(&peer).await?;
            sleep(Duration::from_millis(300)).await;
            assert!(second.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn retries_after_a_failed_attempt() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    // 저장된 인증서와 다른 신원으로 응답하는 피어라서 첫 접속은 인증서 검증에 실패한다.
    let peer = fixture.bind_peer(&new_identity()?)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let mut rejected = 0;
            let conn = loop {
                match timeout(TIMEOUT, rfsync_network::accept_quic(&peer)).await? {
                    Ok(conn) => break conn,
                    Err(_) => {
                        rejected += 1;
                        // 이제 올바른 신원으로 응답해서 다음 재시도가 성공하게 한다.
                        peer.set_server_config(Some(rfsync_network::server_config(
                            &fixture.peer.cert,
                            &fixture.peer.key,
                            &[&fixture.daemon.cert],
                        )?));
                    }
                }
            };

            assert!(rejected >= 1, "첫 접속이 실패하지 않았다");
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn fails_when_no_device_is_paired() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;

    let result = fixture.run(std::future::pending()).await;

    assert!(matches!(result, Err(EngineError::NoPairedDevices)));
    Ok(())
}

#[tokio::test]
async fn keeps_a_paired_peers_connection_open() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        // 데몬이 연결 핸들을 놓았다면 이 시간 안에 연결이 닫힌다.
        sleep(Duration::from_millis(300)).await;
        assert!(conn.close_reason().is_none());
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn keeps_accepting_after_rejecting_an_unpaired_peer() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let unpaired = new_identity()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        // TLS 1.3에서는 클라이언트 쪽 핸드셰이크가 먼저 끝날 수 있으므로,
        // 접속이 성공했더라도 곧 닫히는지로 거부를 확인한다.
        if let Ok((_endpoint, conn)) = fixture.connect(&unpaired).await {
            timeout(TIMEOUT, conn.closed()).await?;
        }

        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        sleep(Duration::from_millis(300)).await;
        assert!(conn.close_reason().is_none());
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn closes_connections_when_shut_down() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        let _ = stop.send(());
        // idle timeout(30초)보다 짧은 시간 안에 닫히므로 종료 처리로 닫힌 것이다.
        timeout(TIMEOUT, conn.closed()).await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
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

/// 루트 하나를 등록한 DB와 그 목록이 만드는 기대 메시지를 준비한다.
fn register_root_with_files(
    fixture: &Fixture,
) -> Result<(tempfile::TempDir, Message), Box<dyn Error>> {
    let root_dir = tempfile::tempdir()?;
    std::fs::write(root_dir.path().join("a.txt"), "hello")?;
    std::fs::create_dir(root_dir.path().join("sub"))?;
    std::fs::write(root_dir.path().join("sub/b.txt"), "world")?;
    crate::register_root(root_dir.path(), &fixture.db_path)?;
    let expected = index_message(load_index(&fixture.db_path)?);
    Ok((root_dir, expected))
}

#[tokio::test]
async fn sends_hello_then_index_to_a_peer_that_connects_to_the_daemon() -> Result<(), Box<dyn Error>>
{
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (_root, expected) = register_root_with_files(&fixture)?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;

            let (first, second) = read_handshake(&mut recv).await?;

            assert_eq!(first, hello());
            assert_eq!(second, expected);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn sends_hello_then_index_to_a_peer_the_daemon_dials() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;
    let (_root, expected) = register_root_with_files(&fixture)?;

    fixture
        .with_daemon(|| async {
            let conn = accept_from_daemon(&peer).await?;
            let (mut send, mut recv) = timeout(TIMEOUT, accept_stream(&conn)).await??;
            write_handshake(&mut send).await?;

            let (first, second) = read_handshake(&mut recv).await?;

            assert_eq!(first, hello());
            assert_eq!(second, expected);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn sends_an_empty_index_when_no_root_is_registered() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;

            let (_, second) = read_handshake(&mut recv).await?;

            assert_eq!(second, index_message(Vec::new()));
            Ok(())
        })
        .await
}

#[tokio::test]
async fn keeps_the_connection_open_after_the_handshake() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;
            read_handshake(&mut recv).await?;

            // 핸드셰이크가 끝나도 데몬이 연결을 닫으면 안 된다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn keeps_the_connection_open_when_the_peer_sends_an_unexpected_message()
-> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, _recv) = open_stream(&conn).await?;
            // Hello가 와야 할 자리에 목록을 보내 핸드셰이크를 실패시킨다.
            write_frame(&mut send, &index_message(Vec::new())).await?;

            // 핸드셰이크가 실패해도 연결 자체는 닫지 않는다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

/// 한 방향으로 흐르는 메모리 통로 한 쌍(쓰는 쪽, 읽는 쪽)을 만든다.
fn channel() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(64 * 1024)
}

/// `exchange`를 제한 시간 안에 실행한다. 구현이 잘못돼 서로 기다리면 멈추지 않고 실패하게 한다.
async fn exchange_in_time(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    index: &[Entry],
) -> Result<PeerHandshake, EngineError> {
    timeout(TIMEOUT, exchange(send, recv, index))
        .await
        .expect("제한 시간 안에 끝나야 한다")
}

#[tokio::test]
async fn exchange_gives_each_side_the_others_index() {
    let (mut a_to_b_write, mut a_to_b_read) = channel();
    let (mut b_to_a_write, mut b_to_a_read) = channel();
    let a_index = vec![entry("a-only.txt")];
    let b_index = vec![entry("b-only.txt"), entry("sub/b.txt")];

    let (a_got, b_got) = tokio::try_join!(
        exchange_in_time(&mut a_to_b_write, &mut b_to_a_read, &a_index),
        exchange_in_time(&mut b_to_a_write, &mut a_to_b_read, &b_index),
    )
    .unwrap();

    assert_eq!(a_got.index, b_index);
    assert_eq!(b_got.index, a_index);
    assert_eq!(a_got.version, PROTOCOL_VERSION);
    assert_eq!(b_got.version, PROTOCOL_VERSION);
}

#[tokio::test]
async fn exchange_reports_the_peers_protocol_version_without_failing() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(
        &mut peer_write,
        &Message::Hello {
            version: PROTOCOL_VERSION + 1,
        },
    )
    .await
    .unwrap();
    write_frame(&mut peer_write, &index_message(Vec::new()))
        .await
        .unwrap();

    let peer = exchange_in_time(&mut my_write, &mut my_read, &[])
        .await
        .unwrap();

    assert_eq!(peer.version, PROTOCOL_VERSION + 1);
}

#[tokio::test]
async fn exchange_rejects_a_first_message_that_is_not_hello() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(&mut peer_write, &index_message(Vec::new()))
        .await
        .unwrap();

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(matches!(
        result,
        Err(EngineError::UnexpectedMessage("Hello"))
    ));
}

#[tokio::test]
async fn exchange_rejects_a_second_message_that_is_not_an_index() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(&mut peer_write, &hello()).await.unwrap();
    write_frame(&mut peer_write, &hello()).await.unwrap();

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(matches!(
        result,
        Err(EngineError::UnexpectedMessage("Index"))
    ));
}

#[tokio::test]
async fn exchange_fails_when_the_peer_closes_before_sending() {
    let (mut my_write, _peer_read) = channel();
    let (peer_write, mut my_read) = channel();
    drop(peer_write);

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn exchange_fails_instead_of_sending_an_index_over_the_frame_limit() {
    let (mut my_write, _peer_read) = channel();
    let (_peer_write, mut my_read) = channel();
    // 항목 하나가 약 120바이트라 1 MiB 상한을 넘기려면 만 개를 훌쩍 넘겨야 한다.
    let too_many: Vec<Entry> = (0..20_000)
        .map(|i| entry(&format!("dir/{i:0>100}.txt")))
        .collect();

    let result = exchange_in_time(&mut my_write, &mut my_read, &too_many).await;

    assert!(matches!(
        result,
        Err(EngineError::Protocol(
            rfsync_protocol::ProtocolError::FrameTooLarge { .. }
        ))
    ));
}

#[tokio::test]
async fn starts_and_serves_when_a_root_is_registered() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let root_dir = tempfile::tempdir()?;
    std::fs::write(root_dir.path().join("a.txt"), "hello")?;
    crate::register_root(root_dir.path(), &fixture.db_path)?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

/// 테스트에서 `tracing` 로그를 모아 보는 버퍼.
#[derive(Clone, Default)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// 원하는 문구가 로그에 나올 때까지 기다린다. 나오지 않으면 지금까지의 로그와 함께 실패한다.
    async fn wait_for(&self, text: &str) {
        let found = timeout(TIMEOUT, async {
            while !self.contents().contains(text) {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            found.is_ok(),
            "로그에 {text:?}가 없다:\n{}",
            self.contents()
        );
    }
}

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

/// 이 테스트가 끝날 때까지 현재 스레드의 로그를 버퍼로 보낸다. 테스트 런타임은 단일 스레드라 데몬의 작업도 잡힌다.
fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

#[tokio::test]
async fn compares_the_peers_index_with_its_own_after_the_handshake() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    // 내 목록: a.txt("hello"), sub/b.txt("world")
    let (_root, _) = register_root_with_files(&fixture)?;
    let (logs, _guard) = capture_logs();

    // 상대 목록: a.txt는 같고, sub/b.txt는 내용이 다르고, new.txt는 내게 없다.
    let mut same = entry("a.txt");
    same.hash = *blake3::hash(b"hello").as_bytes();
    let peer_index = vec![same, entry("sub/b.txt"), entry("new.txt")];

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, _recv) = open_stream(&conn).await?;
            write_frame(&mut send, &hello()).await?;
            write_frame(&mut send, &index_message(peer_index)).await?;

            logs.wait_for("내게 없는 파일 1개, 내용이 다른 파일 1개")
                .await;
            Ok(())
        })
        .await?;

    let log = logs.contents();
    assert!(log.contains("내게 없는 파일: new.txt"), "{log}");
    assert!(log.contains("내용이 다른 파일: sub/b.txt"), "{log}");
    assert!(!log.contains("내게 없는 파일: a.txt"), "{log}");
    Ok(())
}
