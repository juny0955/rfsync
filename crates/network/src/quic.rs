use std::{net::SocketAddr, sync::Arc};

use quinn::{
    Connection, Endpoint,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    rustls::{
        self, RootCertStore,
        pki_types::{CertificateDer, PrivateKeyDer},
        server::WebPkiClientVerifier,
    },
};

use crate::NetworkError;

pub async fn connect(
    addr: SocketAddr,
    my_cert: CertificateDer<'static>,
    my_key: PrivateKeyDer<'static>,
    peer_cert: CertificateDer<'static>,
) -> Result<(Endpoint, Connection), NetworkError> {
    let mut roots = RootCertStore::empty();
    roots.add(peer_cert)?;
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;

    let tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![my_cert], my_key)?;
    let config = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));

    let endpoint = Endpoint::server(config, addr)?;
    let incoming = endpoint
        .accept()
        .await
        .ok_or(NetworkError::QuicEndpointClosed)?;
    let connection = incoming.await?;

    Ok((endpoint, connection))
}

pub async fn accept(
    addr: SocketAddr,
    server_addr: SocketAddr,
    server_name: &str,
    my_cert: CertificateDer<'static>,
    my_key: PrivateKeyDer<'static>,
    peer_cert: CertificateDer<'static>,
) -> Result<(Endpoint, Connection), NetworkError> {
    let mut roots = RootCertStore::empty();
    roots.add(peer_cert)?;

    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![my_cert], my_key)?;
    let config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));

    let endpoint = Endpoint::client(addr)?;
    let connection = endpoint
        .connect_with(config, server_addr, server_name)?
        .await?;

    Ok((endpoint, connection))
}

#[cfg(test)]
mod tests {
    use std::{error::Error, net::UdpSocket};

    use quinn::{
        ConnectionError, TransportErrorCode,
        rustls::{AlertDescription, pki_types::PrivatePkcs8KeyDer},
    };
    use tokio::time::{Duration, timeout};

    use super::{Connection, Endpoint, NetworkError, SocketAddr, accept, connect};
    use crate::cert::generate_certificate;

    const LOOPBACK: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);
    const SERVER_NAME: &str = "server.local";
    const CLIENT_NAME: &str = "client.local";
    const TIMEOUT: Duration = Duration::from_secs(5);

    type Handshake = Result<(Endpoint, Connection), NetworkError>;

    async fn handshake(
        (client_cert, client_key): (Vec<u8>, Vec<u8>),
        trusted_client_cert: Vec<u8>,
    ) -> Result<(Handshake, Handshake), Box<dyn Error>> {
        let (server_cert, server_key) = generate_certificate(SERVER_NAME)?;
        // Release the OS-assigned port so the server API can bind it itself.
        let server_addr = UdpSocket::bind(LOOPBACK)?.local_addr()?;

        Ok(timeout(TIMEOUT, async {
            tokio::join!(
                biased;
                connect(
                    server_addr,
                    server_cert.clone().into(),
                    PrivatePkcs8KeyDer::from(server_key).into(),
                    trusted_client_cert.into(),
                ),
                accept(
                    LOOPBACK,
                    server_addr,
                    SERVER_NAME,
                    client_cert.into(),
                    PrivatePkcs8KeyDer::from(client_key).into(),
                    server_cert.into(),
                ),
            )
        })
        .await?)
    }

    #[tokio::test]
    async fn paired_peers_exchange_data_over_quic() -> Result<(), Box<dyn Error>> {
        let client_identity = generate_certificate(CLIENT_NAME)?;
        let trusted_client = client_identity.0.clone();
        let (server, client) = handshake(client_identity, trusted_client).await?;
        let (_server_endpoint, server) = server?;
        let (_client_endpoint, client) = client?;

        timeout(TIMEOUT, async {
            let server = async {
                let (mut send, mut recv) = server.accept_bi().await?;
                assert_eq!(recv.read_to_end(4).await?, b"ping");
                send.write_all(b"pong").await?;
                send.finish()?;
                Ok::<(), Box<dyn Error>>(())
            };
            let client = async {
                let (mut send, mut recv) = client.open_bi().await?;
                send.write_all(b"ping").await?;
                send.finish()?;
                assert_eq!(recv.read_to_end(4).await?, b"pong");
                Ok(())
            };
            tokio::try_join!(server, client)
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn server_rejects_unpaired_client_certificate() -> Result<(), Box<dyn Error>> {
        let (trusted_client, _) = generate_certificate(CLIENT_NAME)?;
        let unpaired_client = generate_certificate(CLIENT_NAME)?;
        let (server, _client) = handshake(unpaired_client, trusted_client).await?;

        let error = server.expect_err("accepted a client with an unpaired key");
        assert!(
            matches!(&error,
                NetworkError::QuicConnection(ConnectionError::TransportError(transport))
                    if transport.code == TransportErrorCode::crypto(u8::from(AlertDescription::DecryptError))
            ),
            "{error}"
        );
        Ok(())
    }
}
