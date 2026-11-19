use std::net::SocketAddr;

use pakery_spake2::Spake2Output;
use ring::{
    hkdf::{self, HKDF_SHA256, Prk},
    hmac,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    NetworkError,
    cert::generate_certificate,
    pairing::{
        spake2::{PartyState, start_a, start_b},
        tcp::{bind_tcp, connect_tcp, receive_message, send_message},
    },
};

mod spake2;
mod tcp;

const A_TO_B: &[u8] = b"rfsync/v1/cert/A-to-B";
const B_TO_A: &[u8] = b"rfsync/v1/cert/B-to-A";

pub async fn listen_pairing(addr: SocketAddr, code: &str) -> Result<(), NetworkError> {
    let (certificate, _private_key) = generate_certificate("local")?;
    let mut stream = bind_tcp(addr).await?;

    let (message, state) = start_a(code)?;
    let output = verify_spake2(&mut stream, &message, state).await?;

    let _peer_cert = exchange_certificate(
        &mut stream,
        &certificate,
        output.session_key.as_bytes(),
        A_TO_B,
        B_TO_A,
    )
    .await?;

    Ok(())
}

pub async fn connect_pairing(addr: SocketAddr, code: &str) -> Result<(), NetworkError> {
    let (certificate, _private_key) = generate_certificate("local")?;
    let mut stream = connect_tcp(addr).await?;

    let (message, state) = start_b(code)?;
    let output = verify_spake2(&mut stream, &message, state).await?;

    let _peer_cert = exchange_certificate(
        &mut stream,
        &certificate,
        output.session_key.as_bytes(),
        B_TO_A,
        A_TO_B,
    )
    .await?;

    Ok(())
}

pub fn generate_pairing_code() -> String {
    let number = rand::random_range(0..100_000_000u32);
    format!("{number:08}")
}

async fn verify_spake2(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    message: &[u8],
    state: PartyState,
) -> Result<Spake2Output, NetworkError> {
    let peer_message = exchange_messge(stream, message).await?;

    let output = state.finish(&peer_message)?;

    let confirmation = exchange_messge(stream, &output.confirmation_mac).await?;
    output.verify_peer_confirmation(&confirmation)?;

    Ok(output)
}

async fn exchange_certificate(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    certificate: &[u8],
    session_key: &[u8],
    send_label: &[u8],
    receive_label: &[u8],
) -> Result<Vec<u8>, NetworkError> {
    let prk = hkdf::Salt::new(HKDF_SHA256, &[]).extract(session_key);

    let send_key = derive_key(&prk, send_label)?;
    let receive_key = derive_key(&prk, receive_label)?;

    let tag = hmac::sign(&send_key, certificate);
    let mut frame = Vec::with_capacity(certificate.len() + 32);
    frame.extend_from_slice(certificate);
    frame.extend_from_slice(tag.as_ref());

    let mut peer_frame = exchange_messge(stream, &frame).await?;
    let (peer_der, peer_tag) = peer_frame
        .split_last_chunk::<32>()
        .filter(|(der, _)| !der.is_empty())
        .ok_or(NetworkError::PeerCertificateAuthentication)?;

    hmac::verify(&receive_key, peer_der, peer_tag)
        .map_err(|_| NetworkError::PeerCertificateAuthentication)?;

    peer_frame.truncate(peer_der.len());
    Ok(peer_frame)
}

async fn exchange_messge(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    message: &[u8],
) -> Result<Vec<u8>, NetworkError> {
    send_message(stream, message).await?;
    let recived = receive_message(stream).await?;
    Ok(recived)
}

fn derive_key(prk: &Prk, label: &[u8]) -> Result<hmac::Key, NetworkError> {
    prk.expand(&[label], hmac::HMAC_SHA256)
        .map(hmac::Key::from)
        .map_err(|_| NetworkError::PairingKeyDerivation)
}

#[cfg(test)]
mod tests {
    use std::{error::Error, io::ErrorKind};

    use super::{
        A_TO_B, B_TO_A, connect_pairing, exchange_certificate, generate_certificate,
        generate_pairing_code, listen_pairing, receive_message, send_message, start_a,
    };
    use crate::NetworkError;
    use tokio::{
        io::duplex,
        net::TcpListener,
        time::{Duration, timeout},
    };

    async fn exchange_with_client(
        server_code: &str,
        client_code: &str,
    ) -> Result<(Result<(), NetworkError>, Result<(), NetworkError>), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;

        let server = async {
            let (mut stream, _) = listener.accept().await?;
            let (message, state) = start_a(server_code)?;
            send_message(&mut stream, &message).await?;
            let peer_message = receive_message(&mut stream).await?;
            let output = state.finish(&peer_message)?;
            send_message(&mut stream, &output.confirmation_mac).await?;
            let confirmation = receive_message(&mut stream).await?;
            output.verify_peer_confirmation(&confirmation)?;
            let (certificate, _) = generate_certificate("local")?;
            exchange_certificate(
                &mut stream,
                &certificate,
                output.session_key.as_bytes(),
                A_TO_B,
                B_TO_A,
            )
            .await?;
            Ok::<(), NetworkError>(())
        };

        Ok(timeout(Duration::from_secs(5), async {
            tokio::join!(server, connect_pairing(address, client_code))
        })
        .await?)
    }

    #[tokio::test]
    async fn exchange_certificate_returns_each_peers_der() -> Result<(), Box<dyn Error>> {
        let (mut a, mut b) = duplex(8192);
        let (a_der, _) = generate_certificate("a.local")?;
        let (b_der, _) = generate_certificate("b.local")?;
        let session_key = [42; 32];

        let (received_by_a, received_by_b) = timeout(Duration::from_secs(5), async {
            tokio::try_join!(
                exchange_certificate(&mut a, &a_der, &session_key, A_TO_B, B_TO_A),
                exchange_certificate(&mut b, &b_der, &session_key, B_TO_A, A_TO_B),
            )
        })
        .await??;

        assert_eq!(received_by_a, b_der);
        assert_eq!(received_by_b, a_der);
        Ok(())
    }

    #[tokio::test]
    async fn exchange_certificate_rejects_modified_der() -> Result<(), Box<dyn Error>> {
        let (mut a, mut relay_a) = duplex(8192);
        let (mut b, mut relay_b) = duplex(8192);
        let (a_der, _) = generate_certificate("a.local")?;
        let (b_der, _) = generate_certificate("b.local")?;
        let session_key = [42; 32];

        let relay = async {
            let from_a = receive_message(&mut relay_a).await?;
            let mut from_b = receive_message(&mut relay_b).await?;
            from_b[0] ^= 1;
            send_message(&mut relay_b, &from_a).await?;
            send_message(&mut relay_a, &from_b).await
        };

        let (received_by_a, received_by_b, relayed) = timeout(Duration::from_secs(5), async {
            tokio::join!(
                exchange_certificate(&mut a, &a_der, &session_key, A_TO_B, B_TO_A),
                exchange_certificate(&mut b, &b_der, &session_key, B_TO_A, A_TO_B),
                relay,
            )
        })
        .await?;

        relayed?;
        assert_eq!(received_by_b?, a_der);
        assert!(matches!(
            received_by_a,
            Err(NetworkError::PeerCertificateAuthentication)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn connect_pairing_confirms_both_peers_when_codes_match() -> Result<(), Box<dyn Error>> {
        let (server, client) = exchange_with_client("00123456", "00123456").await?;

        server?;
        client?;
        Ok(())
    }

    #[tokio::test]
    async fn connect_pairing_rejects_both_peers_when_codes_differ() -> Result<(), Box<dyn Error>> {
        let (server, client) = exchange_with_client("00123456", "00123457").await?;

        assert!(matches!(
            server,
            Err(NetworkError::Spake2(
                pakery_spake2::Spake2Error::ConfirmationFailed
            ))
        ));
        assert!(matches!(
            client,
            Err(NetworkError::Spake2(
                pakery_spake2::Spake2Error::ConfirmationFailed
            ))
        ));
        Ok(())
    }

    #[test]
    fn pairing_code_contains_eight_ascii_digits() {
        let code = generate_pairing_code();

        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|digit| digit.is_ascii_digit()));
    }

    #[tokio::test]
    async fn listen_pairing_reports_an_address_that_is_already_in_use() -> Result<(), Box<dyn Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;

        let error = listen_pairing(address, "00123456").await.unwrap_err();

        assert!(matches!(error, NetworkError::Io(error) if error.kind() == ErrorKind::AddrInUse));
        Ok(())
    }
}
