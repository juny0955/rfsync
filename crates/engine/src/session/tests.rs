use std::{
    error::Error,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    time::Duration,
};

use rfsync_network::{Endpoint, QuicConnection, generate_certificate};
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};

use super::*;
use crate::identity::SERVER_NAME;

const TIMEOUT: Duration = Duration::from_secs(10);

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
        // 비어 있는 UDP 포트를 찾아서 쓴다.
        let port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port();
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

/// 상대가 연 uni 스트림에서 첫 메시지를 읽는다.
async fn read_hello(conn: &QuicConnection) -> Result<Message, Box<dyn Error>> {
    let mut recv = timeout(TIMEOUT, conn.accept_uni()).await??;
    Ok(timeout(TIMEOUT, read_frame(&mut recv)).await??)
}

#[tokio::test]
async fn sends_hello_to_a_peer_that_connects_to_the_daemon() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            assert_eq!(read_hello(&conn).await?, hello());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn sends_hello_to_a_peer_the_daemon_dials() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let conn = accept_from_daemon(&peer).await?;

            assert_eq!(read_hello(&conn).await?, hello());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn keeps_the_connection_open_after_exchanging_hello() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            // 데몬의 Hello를 받고, 우리도 Hello를 보낸다.
            assert_eq!(read_hello(&conn).await?, hello());
            let mut send = conn.open_uni().await?;
            write_frame(&mut send, &hello()).await?;
            let _ = send.finish();

            // Hello 교환이 끝나도 데몬이 연결을 닫으면 안 된다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}
