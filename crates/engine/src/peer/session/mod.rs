use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use rfsync_network::{ClientConfig, Endpoint, NetworkError, QuicConnection};
use rfsync_protocol::{Entry, Message, PROTOCOL_VERSION, read_frame, write_frame};
use tokio::{task::JoinSet, time::sleep};
use tracing::{debug, info, warn};

use self::backoff::Backoff;
use super::{
    identity::{Identity, SERVER_NAME},
    index::load_index,
};
use crate::error::EngineError;

mod backoff;

/// 연결에서 이 쪽이 맡은 역할. 연결을 시작한 쪽이 스트림을 열고, 받은 쪽이 스트림을 받는다.
#[derive(Debug, Clone, Copy)]
enum SessionRole {
    Dialer,
    Acceptor,
}

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

    // 임시: 시작 시점의 목록을 모든 연결에 보낸다. 라이브 변경 반영은 coordinator 도입 때 처리한다.
    let index = Arc::new(load_index(db_path)?);

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
            warn!("저장된 주소가 올바르지 않습니다: {addr}");
            continue;
        };

        let config =
            rfsync_network::client_config(&identity.cert, &identity.key, &device.cert_der)?;
        dials.spawn(dial_loop(endpoint.clone(), addr, config, index.clone()));
    }

    tokio::select! {
        _ = shutdown => {}
        _ = accept_loop(&endpoint, index) => {}
    }
    endpoint.close(0u32.into(), b"shutdown");
    dials.shutdown().await;
    endpoint.wait_idle().await;

    Ok(())
}

async fn dial_loop(
    endpoint: Endpoint,
    addr: SocketAddr,
    config: ClientConfig,
    index: Arc<Vec<Entry>>,
) {
    let mut backoff = Backoff::new();

    loop {
        match rfsync_network::connect_quic(&endpoint, addr, SERVER_NAME, config.clone()).await {
            Ok(conn) => {
                backoff.reset();
                hold_connection(conn, SessionRole::Dialer, index.clone()).await;
            }
            Err(e) => warn!("{addr} 연결 실패: {e}"),
        }
        sleep(backoff.next_delay()).await;
    }
}

async fn accept_loop(endpoint: &Endpoint, index: Arc<Vec<Entry>>) {
    loop {
        match rfsync_network::accept_quic(endpoint).await {
            Ok(conn) => {
                tokio::spawn(hold_connection(conn, SessionRole::Acceptor, index.clone()));
            }
            Err(NetworkError::QuicEndpointClosed) => return,
            Err(e) => warn!("연결 수락 실패: {e}"),
        }
    }
}

async fn hold_connection(conn: QuicConnection, role: SessionRole, index: Arc<Vec<Entry>>) {
    debug!("연결 시작: {role:?}, 보낼 파일 목록 {}개", index.len());

    if let Err(e) = send_hello(&conn).await {
        warn!("Hello 전송 실패: {e}");
    }

    match recv_hello(&conn).await {
        Ok(msg) => debug!("수신 메시지: {msg:?}"),
        Err(e) => warn!("Hello 수신 실패: {e}"),
    }

    let reason = conn.closed().await;
    info!("연결 종료: {reason}");
}

async fn send_hello(conn: &QuicConnection) -> Result<(), EngineError> {
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

async fn recv_hello(conn: &QuicConnection) -> Result<Message, EngineError> {
    let mut recv = conn.accept_uni().await.map_err(NetworkError::from)?;

    let msg = read_frame(&mut recv).await?;
    Ok(msg)
}

#[cfg(test)]
mod tests;
