use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_network::{Endpoint, QuicConnection};
use tracing::info;

use super::identity::{SERVER_NAME, load_or_create_identity};
use crate::error::EngineError;

/// 페어링을 기다린다. 코드를 만들면 `on_code`로 건네며, 사용자에게 보여주는 일은 호출하는 쪽이 한다.
pub async fn listen(
    port: u16,
    db_path: &Path,
    on_code: impl FnOnce(&str),
) -> Result<(rfsync_network::Endpoint, QuicConnection), EngineError> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let identity = load_or_create_identity()?;

    let code = rfsync_network::generate_pairing_code();
    on_code(&code);

    let peer_cert = rfsync_network::listen_pairing(bind_addr, &code, &identity.cert).await?;
    info!("A: 상대 키 확인 성공");

    let config = rfsync_network::server_config(&identity.cert, &identity.key, &[&peer_cert])?;
    let endpoint = rfsync_network::bind_quic(bind_addr, config)?;
    let connection = rfsync_network::accept_quic(&endpoint).await?;
    save_device(db_path, &peer_cert, None)?;
    info!("A: QUIC 연결 성공");
    Ok((endpoint, connection))
}

pub async fn connect(
    port: u16,
    server_addr: SocketAddr,
    code: &str,
    db_path: &Path,
) -> Result<(Endpoint, QuicConnection), EngineError> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let identity = load_or_create_identity()?;
    let peer_cert = rfsync_network::connect_pairing(server_addr, code, &identity.cert).await?;
    info!("B: 상대 키 확인 성공");

    let server_config =
        rfsync_network::server_config(&identity.cert, &identity.key, &[&peer_cert])?;
    let endpoint = rfsync_network::bind_quic(bind_addr, server_config)?;
    let client_config = rfsync_network::client_config(&identity.cert, &identity.key, &peer_cert)?;
    let connection =
        rfsync_network::connect_quic(&endpoint, server_addr, SERVER_NAME, client_config).await?;
    save_device(db_path, &peer_cert, Some(server_addr))?;
    info!("B: QUIC 연결 성공");
    Ok((endpoint, connection))
}

/// 데몬용. 이미 만든 코드로 상대의 접속을 기다려 키를 교환하고 상대를 저장한다.
/// QUIC 연결은 맺지 않는다. 데몬의 엔드포인트가 저장된 장치를 보고 맡는다.
pub(crate) async fn pair_listen(
    port: u16,
    cert: &[u8],
    code: &str,
    db_path: &Path,
) -> Result<(), EngineError> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let peer_cert = rfsync_network::listen_pairing(bind_addr, code, cert).await?;
    save_device(db_path, &peer_cert, None)?;
    info!("A: 상대 키 확인 성공");
    Ok(())
}

/// 데몬용. 상대의 주소와 코드로 키를 교환하고, 상대의 주소와 함께 저장한다.
/// 저장된 주소로 데몬이 연결을 건다.
pub(crate) async fn pair_connect(
    server_addr: SocketAddr,
    cert: &[u8],
    code: &str,
    db_path: &Path,
) -> Result<(), EngineError> {
    let peer_cert = rfsync_network::connect_pairing(server_addr, code, cert).await?;
    save_device(db_path, &peer_cert, Some(server_addr))?;
    info!("B: 상대 키 확인 성공");
    Ok(())
}

fn save_device(
    db_path: &Path,
    cert_der: &[u8],
    endpoint: Option<SocketAddr>,
) -> Result<(), EngineError> {
    let paired_at = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let endpoint = endpoint.map(|addr| addr.to_string());
    let conn = rfsync_storage::open(db_path)?;
    rfsync_storage::upsert_device(&conn, cert_der, endpoint.as_deref(), paired_at)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        error::Error,
        fs, io,
        net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
        path::Path,
        process::Stdio,
        time::Duration,
    };

    use rfsync_network::NetworkError;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        process::Command,
        time::timeout,
    };

    use super::{EngineError, connect, listen};

    const DB_PATH: &str = "pairing.db";

    #[tokio::test]
    async fn pairs_on_separate_ports_and_stores_each_remote_device() -> Result<(), Box<dyn Error>> {
        let listener_dir = tempfile::tempdir()?;
        let dialer_dir = tempfile::tempdir()?;

        pair(listener_dir.path(), dialer_dir.path()).await?;
        let listener_identity = identity_from(listener_dir.path())?;
        let dialer_identity = identity_from(dialer_dir.path())?;
        assert!(listener_identity.0 != dialer_identity.0);

        let server_addr = pair(listener_dir.path(), dialer_dir.path()).await?;

        assert!(identity_from(listener_dir.path())? == listener_identity);
        assert!(identity_from(dialer_dir.path())? == dialer_identity);

        for (dir, peer_cert, endpoint) in [
            (listener_dir.path(), dialer_identity.0, None),
            (
                dialer_dir.path(),
                listener_identity.0,
                Some(server_addr.to_string()),
            ),
        ] {
            let conn = rfsync_storage::open(&dir.join(DB_PATH))?;
            let count: i64 =
                conn.query_row("SELECT count(*) FROM devices", [], |row| row.get(0))?;
            assert_eq!(count, 1);
            let stored: (Vec<u8>, Option<String>, i64) = conn.query_row(
                "SELECT cert_der, endpoint, paired_at FROM devices",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(stored.0, peer_cert);
            assert_eq!(stored.1, endpoint);
            assert!(stored.2 > 0);
        }
        Ok(())
    }

    async fn pair(listener_dir: &Path, dialer_dir: &Path) -> Result<SocketAddr, Box<dyn Error>> {
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server_addr = tcp.local_addr()?;
        let server_udp = UdpSocket::bind(server_addr)?;
        let dialer_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let dialer_port = dialer_udp.local_addr()?.port();
        drop((tcp, server_udp, dialer_udp));

        timeout(Duration::from_secs(10), async {
            let mut server = peer_command(listener_dir, server_addr.port())?
                .stdout(Stdio::piped())
                .spawn()?;
            let stdout = server.stdout.take().ok_or("missing listener stdout")?;
            let mut lines = BufReader::new(stdout).lines();
            let code = loop {
                let line = lines
                    .next_line()
                    .await?
                    .ok_or("listener did not print a code")?;
                if let Some((_, code)) = line.split_once("code: ") {
                    break code.to_owned();
                }
            };
            let mut client = peer_command(dialer_dir, dialer_port)?;
            client
                .env("RFSYNC_TEST_PEER_ADDR", server_addr.to_string())
                .env("RFSYNC_TEST_PAIR_CODE", code);

            let (server, client) = tokio::try_join!(server.wait(), client.output())?;
            assert!(server.success(), "listener exited with {server}");
            assert!(
                client.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&client.stdout),
                String::from_utf8_lossy(&client.stderr),
            );
            Ok::<SocketAddr, Box<dyn Error>>(server_addr)
        })
        .await?
    }

    fn peer_command(dir: &Path, port: u16) -> io::Result<Command> {
        let mut command = Command::new(env::current_exe()?);
        command
            .current_dir(dir)
            .kill_on_drop(true)
            .args([
                "--ignored",
                "--exact",
                "--nocapture",
                "peer::pairing::tests::run_peer",
            ])
            .env("RFSYNC_TEST_PAIR_PORT", port.to_string())
            .env_remove("RFSYNC_TEST_PEER_ADDR");
        Ok(command)
    }

    fn identity_from(dir: &Path) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let dir = dir.join("iden");
        Ok((
            fs::read(dir.join("cert.der"))?,
            fs::read(dir.join("key.der"))?,
        ))
    }

    #[tokio::test]
    #[ignore = "launched in an isolated working directory by the pairing test"]
    async fn run_peer() -> Result<(), Box<dyn Error>> {
        let Ok(port) = env::var("RFSYNC_TEST_PAIR_PORT") else {
            return Ok(());
        };
        let port = port.parse()?;
        let db_path = Path::new(DB_PATH);
        if let Ok(addr) = env::var("RFSYNC_TEST_PEER_ADDR") {
            let addr: SocketAddr = addr.parse()?;
            let code = env::var("RFSYNC_TEST_PAIR_CODE")?;
            loop {
                match connect(port, addr, &code, db_path).await {
                    Err(EngineError::Network(NetworkError::Io(error)))
                        if error.kind() == io::ErrorKind::ConnectionRefused =>
                    {
                        // The listener prints its code just before binding TCP.
                        tokio::task::yield_now().await;
                    }
                    result => {
                        result?;
                        return Ok(());
                    }
                }
            }
        }
        // 부모 테스트가 이 프로세스의 stdout에서 코드를 읽는다. CLI와 같은 형식으로 출력한다.
        listen(port, db_path, |code| println!("code: {code}")).await?;
        Ok(())
    }
}
