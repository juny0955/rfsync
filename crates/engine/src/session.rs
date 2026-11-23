use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use rfsync_network::{Endpoint, NetworkError};

use crate::{error::EngineError, identity::Identity};

pub async fn run(
    identity: &Identity,
    port: u16,
    db_path: &Path,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    let devices = {
        let conn = rfsync_storage::open(db_path)?;
        rfsync_storage::list_devices(&conn)?
    };

    if devices.is_empty() {
        return Err(EngineError::NoPairedDevices);
    }

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let certs: Vec<&[u8]> = devices.iter().map(|d| d.cert_der.as_slice()).collect();
    let config = rfsync_network::server_config(&identity.cert, &identity.key, &certs)?;

    let endpoint = rfsync_network::bind_quic(addr, config)?;

    tokio::select! {
        _ = shutdown => {}
        _ = accept_loop(&endpoint) => {}
    }
    endpoint.close(0u32.into(), b"shutdown");
    endpoint.wait_idle().await;

    Ok(())
}

async fn accept_loop(endpoint: &Endpoint) {
    loop {
        match rfsync_network::accept_quic(endpoint).await {
            Ok(conn) => {
                tokio::spawn(hold_connection(conn));
            }
            Err(NetworkError::QuicEndpointClosed) => return,
            Err(e) => eprintln!("연결 수락 실패: {e}"),
        }
    }
}

async fn hold_connection(conn: rfsync_network::Connection) {
    let reason = conn.closed().await;
    eprintln!("연결 종료: {reason}");
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        net::{Ipv4Addr, SocketAddr, UdpSocket},
        path::PathBuf,
        time::Duration,
    };

    use rfsync_network::{Connection, Endpoint, generate_certificate};
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
        ) -> Result<(Endpoint, Connection), NetworkError> {
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
}
