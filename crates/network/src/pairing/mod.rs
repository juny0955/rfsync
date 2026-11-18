use std::net::SocketAddr;

use crate::{
    NetworkError,
    pairing::spake2::{start_a, start_b},
    pairing::tcp::{bind_tcp, connect_tcp, receive_message, send_message},
};

mod spake2;
mod tcp;

pub async fn listen_pairing(addr: SocketAddr, code: &str) -> Result<(), NetworkError> {
    let mut stream = bind_tcp(addr).await?;

    let (message, state) = start_a(code)?;
    send_message(&mut stream, &message).await?;
    let message_b = receive_message(&mut stream).await?;

    let output_a = state.finish(&message_b)?;

    send_message(&mut stream, &output_a.confirmation_mac).await?;
    let confirmation_b = receive_message(&mut stream).await?;
    output_a.verify_peer_confirmation(&confirmation_b)?;

    Ok(())
}

pub async fn connect_pairing(addr: SocketAddr, code: &str) -> Result<(), NetworkError> {
    let mut stream = connect_tcp(addr).await?;

    let (message, state) = start_b(code)?;
    send_message(&mut stream, &message).await?;
    let message_a = receive_message(&mut stream).await?;

    let output_b = state.finish(&message_a)?;

    send_message(&mut stream, &output_b.confirmation_mac).await?;
    let confirmation_a = receive_message(&mut stream).await?;
    output_b.verify_peer_confirmation(&confirmation_a)?;

    Ok(())
}

pub fn generate_pairing_code() -> String {
    let number = rand::random_range(0..100_000_000u32);
    format!("{number:08}")
}

#[cfg(test)]
mod tests {
    use std::{error::Error, io::ErrorKind};

    use super::{
        connect_pairing, generate_pairing_code, listen_pairing, receive_message, send_message,
        start_a,
    };
    use crate::NetworkError;
    use tokio::{
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
            Ok::<(), NetworkError>(())
        };

        Ok(timeout(Duration::from_secs(5), async {
            tokio::join!(server, connect_pairing(address, client_code))
        })
        .await?)
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
