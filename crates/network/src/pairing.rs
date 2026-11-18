use std::net::SocketAddr;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::NetworkError;

const MAX_MESSAGE_SIZE: usize = 4096;

pub async fn send_message(stream: &mut TcpStream, message: &[u8]) -> Result<(), NetworkError> {
    if message.len() > MAX_MESSAGE_SIZE {
        return Err(NetworkError::MessageTooLarge {
            max: MAX_MESSAGE_SIZE,
            size: message.len(),
        });
    }

    let length = u16::try_from(message.len()).map_err(|_| NetworkError::MessageTooLarge {
        max: MAX_MESSAGE_SIZE,
        size: message.len(),
    })?;

    stream.write_u16(length).await?;
    stream.write_all(message).await?;

    Ok(())
}

pub async fn receive_message(stream: &mut TcpStream) -> Result<Vec<u8>, NetworkError> {
    let length = usize::from(stream.read_u16().await?);

    if length > MAX_MESSAGE_SIZE {
        return Err(NetworkError::MessageTooLarge {
            max: MAX_MESSAGE_SIZE,
            size: length,
        });
    }

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

pub fn generate_pairing_code() -> String {
    let number = rand::random_range(0..100_000_000u32);
    format!("{number:08}")
}
