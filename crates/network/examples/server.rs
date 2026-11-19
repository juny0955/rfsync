use std::{error::Error, io, net::SocketAddr, time::Duration};

use quinn::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rfsync_network::{
    connect as accept_quic, generate_certificate, generate_pairing_code, listen_pairing,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let (cert, key) = generate_certificate("test-server")?;
    let addr = SocketAddr::from(([127, 0, 0, 1], 10000));

    let code = generate_pairing_code();
    println!("code: {code}");
    let peer_cert =
        tokio::time::timeout(Duration::from_secs(120), listen_pairing(addr, &code, &cert))
            .await??;

    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, connection) = accept_quic(
            addr,
            CertificateDer::from(cert),
            PrivatePkcs8KeyDer::from(key).into(),
            CertificateDer::from(peer_cert),
        )
        .await?;
        let (mut send, mut recv) = connection.accept_bi().await?;
        if recv.read_to_end(4).await? != b"ping" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "expected ping").into());
        }
        println!("QUIC received: ping");

        send.write_all(b"pong").await?;
        send.finish()?;
        send.stopped().await?;
        connection.close(0u32.into(), b"done");
        endpoint.wait_idle().await;
        Ok(())
    })
    .await?
}
