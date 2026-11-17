use std::net::SocketAddr;

use tokio::{
    io, net::{TcpListener, TcpStream},
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
