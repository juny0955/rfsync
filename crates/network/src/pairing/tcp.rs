use std::net::SocketAddr;

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::NetworkError;

const MAX_MESSAGE_SIZE: usize = 4096;

pub async fn send_message(
    stream: &mut (impl AsyncWrite + Unpin),
    message: &[u8],
) -> Result<(), NetworkError> {
    validate_message_size(message.len())?;

    let length = u16::try_from(message.len()).map_err(|_| NetworkError::MessageTooLarge {
        max: MAX_MESSAGE_SIZE,
        size: message.len(),
    })?;

    stream.write_u16(length).await?;
    stream.write_all(message).await?;

    Ok(())
}

pub async fn receive_message(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, NetworkError> {
    let length = usize::from(stream.read_u16().await?);
    validate_message_size(length)?;

    let mut message = vec![0; length];
    stream.read_exact(&mut message).await?;

    Ok(message)
}

pub async fn bind_tcp(addr: SocketAddr) -> Result<TcpStream, NetworkError> {
    let listener = TcpListener::bind(addr).await?;
    let (stream, _peer_addr) = listener.accept().await?;
    Ok(stream)
}

pub async fn connect_tcp(addr: SocketAddr) -> Result<TcpStream, NetworkError> {
    Ok(TcpStream::connect(addr).await?)
}

fn validate_message_size(length: usize) -> Result<(), NetworkError> {
    if length > MAX_MESSAGE_SIZE {
        return Err(NetworkError::MessageTooLarge {
            max: MAX_MESSAGE_SIZE,
            size: length,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, io::ErrorKind};

    use super::{receive_message, send_message};
    use crate::NetworkError;
    use tokio::{
        io::AsyncWriteExt,
        net::{TcpListener, TcpStream},
        time::{Duration, timeout},
    };

    async fn connected_streams() -> Result<(TcpStream, TcpStream), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (client, accepted) = tokio::try_join!(TcpStream::connect(address), listener.accept())?;
        Ok((client, accepted.0))
    }

    #[tokio::test]
    async fn messages_remain_separate_when_sent_over_one_connection() -> Result<(), Box<dyn Error>>
    {
        let (mut sender, mut receiver) = connected_streams().await?;
        let largest = vec![0xa5; 4096];
        let messages: [&[u8]; 4] = [b"hello", b"", b"world\0", &largest];

        timeout(Duration::from_secs(5), async {
            for message in messages {
                send_message(&mut sender, message).await?;
            }
            for message in messages {
                assert_eq!(receive_message(&mut receiver).await?, message);
            }
            Ok::<(), NetworkError>(())
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn sending_rejects_a_message_larger_than_the_limit() -> Result<(), Box<dyn Error>> {
        let (mut sender, _receiver) = connected_streams().await?;

        let error = send_message(&mut sender, &[0; 4097]).await.unwrap_err();

        assert!(matches!(
            error,
            NetworkError::MessageTooLarge {
                size: 4097,
                max: 4096
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn receiving_rejects_an_oversized_length_before_reading_the_body()
    -> Result<(), Box<dyn Error>> {
        let (mut sender, mut receiver) = connected_streams().await?;
        sender.write_u16(4097).await?;

        let error = timeout(Duration::from_secs(5), receive_message(&mut receiver))
            .await?
            .unwrap_err();

        assert!(matches!(
            error,
            NetworkError::MessageTooLarge {
                size: 4097,
                max: 4096
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn receiving_reports_eof_when_the_body_is_truncated() -> Result<(), Box<dyn Error>> {
        let (mut sender, mut receiver) = connected_streams().await?;
        sender.write_u16(3).await?;
        sender.write_all(b"hi").await?;
        sender.shutdown().await?;

        let error = timeout(Duration::from_secs(5), receive_message(&mut receiver))
            .await?
            .unwrap_err();

        assert!(
            matches!(error, NetworkError::Io(error) if error.kind() == ErrorKind::UnexpectedEof)
        );
        Ok(())
    }
}
