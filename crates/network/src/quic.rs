use std::{net::SocketAddr, sync::Arc, time::Duration};

use quinn::{
    ClientConfig, Connection, Endpoint, ServerConfig, TransportConfig, VarInt,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    rustls::{
        self, RootCertStore,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
        server::WebPkiClientVerifier,
    },
};

use crate::NetworkError;

/// 유휴 연결에 keep-alive 패킷을 보내는 주기. idle timeout보다 짧아야 효과가 있다.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
const MAX_IDLE_TIMEOUT_MS: u32 = 30_000;

pub fn bind_quic(addr: SocketAddr, config: ServerConfig) -> Result<Endpoint, NetworkError> {
    Ok(Endpoint::server(config, addr)?)
}

pub async fn accept_quic(endpoint: &Endpoint) -> Result<Connection, NetworkError> {
    let incoming = endpoint
        .accept()
        .await
        .ok_or(NetworkError::QuicEndpointClosed)?;
    Ok(incoming.await?)
}

pub async fn connect_quic(
    endpoint: &Endpoint,
    peer_addr: SocketAddr,
    server_name: &str,
    config: ClientConfig,
) -> Result<Connection, NetworkError> {
    Ok(endpoint
        .connect_with(config, peer_addr, server_name)?
        .await?)
}

pub fn server_config(
    my_cert: &[u8],
    my_key: &[u8],
    trusted_peer_certs: &[&[u8]],
) -> Result<quinn::ServerConfig, NetworkError> {
    server_config_with(
        my_cert,
        my_key,
        trusted_peer_certs,
        default_transport_config(),
    )
}

fn server_config_with(
    my_cert: &[u8],
    my_key: &[u8],
    trusted_peer_certs: &[&[u8]],
    transport: Arc<TransportConfig>,
) -> Result<quinn::ServerConfig, NetworkError> {
    let mut roots = RootCertStore::empty();
    for &peer_cert in trusted_peer_certs {
        roots.add(CertificateDer::from_slice(peer_cert))?;
    }

    let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;
    let cert = CertificateDer::from(my_cert.to_vec());
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(my_key.to_vec()));

    let tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert], key)?;

    let mut config = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    config.transport_config(transport);
    Ok(config)
}

pub fn client_config(
    my_cert: &[u8],
    my_key: &[u8],
    peer_cert: &[u8],
) -> Result<quinn::ClientConfig, NetworkError> {
    client_config_with(my_cert, my_key, peer_cert, default_transport_config())
}

fn client_config_with(
    my_cert: &[u8],
    my_key: &[u8],
    peer_cert: &[u8],
    transport: Arc<TransportConfig>,
) -> Result<quinn::ClientConfig, NetworkError> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from_slice(peer_cert))?;

    let cert = CertificateDer::from(my_cert.to_vec());
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(my_key.to_vec()));

    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![cert], key)?;

    let mut config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    config.transport_config(transport);
    Ok(config)
}

fn transport_config(
    keep_alive_interval: Duration,
    max_idle_timeout_ms: u32,
) -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(keep_alive_interval));
    transport.max_idle_timeout(Some(VarInt::from_u32(max_idle_timeout_ms).into()));
    Arc::new(transport)
}

fn default_transport_config() -> Arc<TransportConfig> {
    transport_config(KEEP_ALIVE_INTERVAL, MAX_IDLE_TIMEOUT_MS)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use quinn::{ConnectionError, TransportErrorCode, rustls::AlertDescription};
    use tokio::time::{Duration, sleep, timeout};

    use super::{
        Arc, Connection, Endpoint, NetworkError, SocketAddr, TransportConfig, VarInt, accept_quic,
        bind_quic, client_config, client_config_with, connect_quic, server_config,
        server_config_with, transport_config,
    };
    use crate::cert::generate_certificate;

    const LOOPBACK: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);
    const SERVER_NAME: &str = "server.local";
    const CLIENT_NAME: &str = "client.local";
    const TIMEOUT: Duration = Duration::from_secs(5);

    type Handshake = (Endpoint, Result<Connection, NetworkError>);

    async fn handshake(
        (client_cert, client_key): (Vec<u8>, Vec<u8>),
        trusted_client_cert: Vec<u8>,
    ) -> Result<(Handshake, Handshake), Box<dyn Error>> {
        let (server_cert, server_key) = generate_certificate(SERVER_NAME)?;
        let server = bind_quic(
            LOOPBACK,
            server_config(&server_cert, &server_key, &[&trusted_client_cert])?,
        )?;
        let client = bind_quic(
            LOOPBACK,
            server_config(&client_cert, &client_key, &[&server_cert])?,
        )?;
        let server_addr = server.local_addr()?;
        let config = client_config(&client_cert, &client_key, &server_cert)?;

        let (accepted, connected) = timeout(TIMEOUT, async {
            tokio::join!(
                accept_quic(&server),
                connect_quic(&client, server_addr, SERVER_NAME, config),
            )
        })
        .await?;
        Ok(((server, accepted), (client, connected)))
    }

    async fn exchange_ping_pong(
        server: Connection,
        client: Connection,
    ) -> Result<(), Box<dyn Error>> {
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

    type Pair = (Endpoint, Connection, Endpoint, Connection);

    async fn connected_pair_with(
        transport: impl Fn() -> Arc<TransportConfig>,
    ) -> Result<Pair, Box<dyn Error>> {
        let (server_cert, server_key) = generate_certificate(SERVER_NAME)?;
        let (client_cert, client_key) = generate_certificate(CLIENT_NAME)?;
        let server = bind_quic(
            LOOPBACK,
            server_config_with(&server_cert, &server_key, &[&client_cert], transport())?,
        )?;
        let client = bind_quic(
            LOOPBACK,
            server_config_with(&client_cert, &client_key, &[&server_cert], transport())?,
        )?;
        let server_addr = server.local_addr()?;
        let config = client_config_with(&client_cert, &client_key, &server_cert, transport())?;

        let (accepted, connected) = timeout(TIMEOUT, async {
            tokio::join!(
                accept_quic(&server),
                connect_quic(&client, server_addr, SERVER_NAME, config),
            )
        })
        .await?;
        Ok((server, accepted?, client, connected?))
    }

    #[tokio::test]
    async fn keep_alive_holds_idle_connection_open() -> Result<(), Box<dyn Error>> {
        let (_server_endpoint, server, _client_endpoint, client) =
            connected_pair_with(|| transport_config(Duration::from_millis(200), 1_000)).await?;

        sleep(Duration::from_millis(2_500)).await;

        assert!(server.close_reason().is_none());
        assert!(client.close_reason().is_none());
        exchange_ping_pong(server, client).await
    }

    #[tokio::test]
    async fn idle_connection_times_out_without_keep_alive() -> Result<(), Box<dyn Error>> {
        let (_server_endpoint, _server, _client_endpoint, client) = connected_pair_with(|| {
            let mut transport = TransportConfig::default();
            transport.max_idle_timeout(Some(VarInt::from_u32(1_000).into()));
            Arc::new(transport)
        })
        .await?;

        let reason = timeout(TIMEOUT, client.closed()).await?;

        assert!(matches!(reason, ConnectionError::TimedOut), "{reason}");
        Ok(())
    }

    #[tokio::test]
    async fn paired_peers_exchange_data_over_quic() -> Result<(), Box<dyn Error>> {
        let client_identity = generate_certificate(CLIENT_NAME)?;
        let trusted_client = client_identity.0.clone();
        let ((_server_endpoint, server), (_client_endpoint, client)) =
            handshake(client_identity, trusted_client).await?;

        exchange_ping_pong(server?, client?).await
    }

    #[tokio::test]
    async fn same_endpoint_accepts_new_peer_while_connected_to_first_peer()
    -> Result<(), Box<dyn Error>> {
        let (a_cert, a_key) = generate_certificate("a.local")?;
        let (b_cert, b_key) = generate_certificate("b.local")?;
        let (c_cert, c_key) = generate_certificate("c.local")?;

        let a = bind_quic(LOOPBACK, server_config(&a_cert, &a_key, &[&b_cert])?)?;
        let b = bind_quic(LOOPBACK, server_config(&b_cert, &b_key, &[&a_cert])?)?;
        let c = bind_quic(LOOPBACK, server_config(&c_cert, &c_key, &[&b_cert])?)?;
        let b_addr = b.local_addr()?;

        let to_a = client_config(&b_cert, &b_key, &a_cert)?;
        let (a_to_b, b_to_a) = timeout(TIMEOUT, async {
            tokio::try_join!(
                accept_quic(&a),
                connect_quic(&b, a.local_addr()?, "a.local", to_a)
            )
        })
        .await??;

        b.set_server_config(Some(server_config(&b_cert, &b_key, &[&a_cert, &c_cert])?));
        let to_b = client_config(&c_cert, &c_key, &b_cert)?;
        let (b_to_c, c_to_b) = timeout(TIMEOUT, async {
            tokio::try_join!(accept_quic(&b), connect_quic(&c, b_addr, "b.local", to_b))
        })
        .await??;

        assert_eq!(a_to_b.remote_address(), b_addr);
        assert_eq!(c_to_b.remote_address(), b_addr);
        tokio::try_join!(
            exchange_ping_pong(a_to_b, b_to_a),
            exchange_ping_pong(b_to_c, c_to_b),
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn server_rejects_unpaired_client_certificate() -> Result<(), Box<dyn Error>> {
        let (trusted_client, _) = generate_certificate(CLIENT_NAME)?;
        let unpaired_client = generate_certificate(CLIENT_NAME)?;
        let ((_server_endpoint, server), _client) =
            handshake(unpaired_client, trusted_client).await?;

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
