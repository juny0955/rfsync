use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use rfsync_network::{ClientConfig, Endpoint, NetworkError};
use rfsync_protocol::{Message, PROTOCOL_VERSION, read_frame, write_frame};
use tokio::{task::JoinSet, time::sleep};

use crate::{
    error::EngineError,
    identity::{Identity, SERVER_NAME},
    session::backoff::Backoff,
};

mod backoff;

pub async fn run(
    identity: &Identity,
    port: u16,
    db_path: &Path,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    let devices = {
        let conn = rfsync_storage::open(db_path)?;
        rfsync_storage::list_devices(&conn)?
    };

    if devices.is_empty() {
        return Err(EngineError::NoPairedDevices);
    }

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let certs: Vec<&[u8]> = devices.iter().map(|d| d.cert_der.as_slice()).collect();
    let config = rfsync_network::server_config(&identity.cert, &identity.key, &certs)?;

    let endpoint = rfsync_network::bind_quic(addr, config)?;

    let mut dials = JoinSet::new();
    for device in &devices {
        let Some(addr) = device.endpoint.as_deref() else {
            continue;
        };

        let Ok(addr) = addr.parse::<SocketAddr>() else {
            eprintln!("저장된 주소가 올바르지 않습니다: {addr}");
            continue;
        };

        let config =
            rfsync_network::client_config(&identity.cert, &identity.key, &device.cert_der)?;
        dials.spawn(dial_loop(endpoint.clone(), addr, config));
    }

    tokio::select! {
        _ = shutdown => {}
        _ = accept_loop(&endpoint) => {}
    }
    endpoint.close(0u32.into(), b"shutdown");
    dials.shutdown().await;
    endpoint.wait_idle().await;

    Ok(())
}

async fn dial_loop(endpoint: Endpoint, addr: SocketAddr, config: ClientConfig) {
    let mut backoff = Backoff::new();

    loop {
        match rfsync_network::connect_quic(&endpoint, addr, SERVER_NAME, config.clone()).await {
            Ok(conn) => {
                backoff.reset();
                hold_connection(conn).await;
            }
            Err(e) => eprintln!("{addr} 연결 실패: {e}"),
        }
        sleep(backoff.next_delay()).await;
    }
}

async fn accept_loop(endpoint: &Endpoint) {
    loop {
        match rfsync_network::accept_quic(endpoint).await {
            Ok(conn) => {
                tokio::spawn(hold_connection(conn));
            }
            Err(NetworkError::QuicEndpointClosed) => return,
            Err(e) => eprintln!("연결 수락 실패: {e}"),
        }
    }
}

async fn hold_connection(conn: rfsync_network::Connection) {
    if let Err(e) = send_hello(&conn).await {
        eprintln!("Hello 전송 실패: {e}");
    }

    match recv_hello(&conn).await {
        Ok(msg) => println!("수신 메세지: {msg:?}"),
        Err(e) => eprintln!("Hello 수신 실패: {e}"),
    }

    let reason = conn.closed().await;
    eprintln!("연결 종료: {reason}");
}

async fn send_hello(conn: &rfsync_network::Connection) -> Result<(), EngineError> {
    let mut send = conn.open_uni().await.map_err(NetworkError::from)?;

    write_frame(
        &mut send,
        &Message::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .await?;
    let _ = send.finish();
    Ok(())
}

async fn recv_hello(conn: &rfsync_network::Connection) -> Result<Message, EngineError> {
    let mut recv = conn.accept_uni().await.map_err(NetworkError::from)?;

    let msg = read_frame(&mut recv).await?;
    Ok(msg)
}

#[cfg(test)]
mod tests;
