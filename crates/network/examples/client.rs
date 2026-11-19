use std::{env, error::Error, io, net::SocketAddr, time::Duration};

use quinn::{
    ConnectionError,
    rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer},
};
use rfsync_network::{accept as connect_quic, connect_pairing, generate_certificate};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let code = env::args().nth(1).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "usage: client <pairing-code>")
    })?;
    let (cert, key) = generate_certificate("test-client")?;
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let server_addr = SocketAddr::from(([127, 0, 0, 1], 10000));

    tokio::time::timeout(Duration::from_secs(10), async {
        let peer_cert = connect_pairing(server_addr, &code, &cert).await?;
        let (_endpoint, connection) = connect_quic(
            addr,
            server_addr,
            "test-server",
            CertificateDer::from(cert),
            PrivatePkcs8KeyDer::from(key).into(),
            CertificateDer::from(peer_cert),
        )
        .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(b"ping").await?;
        send.finish()?;
        if recv.read_to_end(4).await? != b"pong" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "expected pong").into());
        }
        println!("QUIC received: pong");

        match connection.closed().await {
            ConnectionError::ApplicationClosed(close) if close.error_code == 0u32.into() => Ok(()),
            error => Err(error.into()),
        }
    })
    .await?
}
