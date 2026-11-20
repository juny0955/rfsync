use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rfsync_network::{Connection, Endpoint, generate_pairing_code};

use crate::error::EngineError;

const SERVER_NAME: &str = "rfsync";
pub const DEFAULT_PORT: u16 = 10_000;

pub async fn listen(port: u16, db_path: &Path) -> Result<(Endpoint, Connection), EngineError> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let (cert, key) = load_or_create_cert()?;

    let code = generate_pairing_code();
    println!("code: {code}");

    let peer_cert = rfsync_network::listen_pairing(bind_addr, &code, &cert).await?;
    println!("A: 상대 키 확인 성공");

    let config = rfsync_network::server_config(&cert, &key, &[&peer_cert])?;
    let endpoint = rfsync_network::bind_quic(bind_addr, config)?;
    let connection = rfsync_network::accept_quic(&endpoint).await?;
    save_device(db_path, &peer_cert, None)?;
    println!("A: QUIC 연결 성공");
    Ok((endpoint, connection))
}

pub async fn connect(
    port: u16,
    server_addr: SocketAddr,
    code: &str,
    db_path: &Path,
) -> Result<(Endpoint, Connection), EngineError> {
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let (cert, key) = load_or_create_cert()?;
    let peer_cert = rfsync_network::connect_pairing(server_addr, code, &cert).await?;
    println!("B: 상대 키 확인 성공");

    let server_config = rfsync_network::server_config(&cert, &key, &[&peer_cert])?;
    let endpoint = rfsync_network::bind_quic(bind_addr, server_config)?;
    let client_config = rfsync_network::client_config(&cert, &key, &peer_cert)?;
    let connection =
        rfsync_network::connect_quic(&endpoint, server_addr, SERVER_NAME, client_config).await?;
    save_device(db_path, &peer_cert, Some(server_addr))?;
    println!("B: QUIC 연결 성공");
    Ok((endpoint, connection))
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

fn load_or_create_cert() -> Result<(Vec<u8>, Vec<u8>), EngineError> {
    let iden = match rfsync_fs::load_certificate()? {
        Some(iden) => iden,
        None => {
            let (cert, key) = rfsync_network::generate_certificate(SERVER_NAME)?;
            rfsync_fs::save_certificate(&cert, &key)?;
            (cert, key)
        }
    };

    Ok(iden)
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
                "pairing::tests::run_peer",
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
        listen(port, db_path).await?;
        Ok(())
    }
}
