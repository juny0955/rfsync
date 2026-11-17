use std::net::SocketAddr;

use tokio::{
    io,
    net::{TcpListener, TcpStream},
};

pub async fn bind(addr: SocketAddr) -> Result<TcpStream, io::Error> {
    let listener = TcpListener::bind(addr).await?;

    let (stream, _peer_addr) = listener.accept().await?;

    Ok(stream)
}

pub async fn connect(addr: SocketAddr) -> Result<TcpStream, io::Error> {
    let stream = TcpStream::connect(addr).await?;

    Ok(stream)
}

pub fn generate_pairing_code() -> String {
    let number = rand::random_range(0..100_000_000u32);
    format!("{number:08}")
}
