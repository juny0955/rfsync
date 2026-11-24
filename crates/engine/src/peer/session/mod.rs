use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use rfsync_network::{
    ClientConfig, Endpoint, NetworkError, QuicConnection, QuicRecvStream, QuicSendStream,
    accept_stream,
};
use rfsync_protocol::{Entry, Message, PROTOCOL_VERSION, read_frame, write_frame};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc::Sender, oneshot},
    task::JoinSet,
    time::sleep,
};
use tracing::{debug, info, warn};

use self::{backoff::Backoff, handshake::handshake};
use super::identity::{Identity, SERVER_NAME};
use crate::{error::EngineError, peer::PeerEvent};

mod backoff;
mod handshake;

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
    index: Vec<Entry>,
    events: Sender<PeerEvent>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    let devices = {
        let conn = rfsync_storage::open(db_path)?;
        rfsync_storage::list_devices(&conn)?
    };

    if devices.is_empty() {
        return Err(EngineError::NoPairedDevices);
    }

    // 임시: 시작 시점의 목록을 모든 연결에 보낸다. 라이브 변경 반영은 증분 전송 단계에서 처리한다.
    let index = Arc::new(index);

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
        dials.spawn(dial_loop(
            endpoint.clone(),
            addr,
            config,
            index.clone(),
            events.clone(),
        ));
    }

    tokio::select! {
        _ = shutdown => {}
        _ = accept_loop(&endpoint, index, events.clone()) => {}
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
    events: Sender<PeerEvent>,
) {
    let mut backoff = Backoff::new();

    loop {
        match rfsync_network::connect_quic(&endpoint, addr, SERVER_NAME, config.clone()).await {
            Ok(conn) => {
                backoff.reset();
                hold_connection(conn, SessionRole::Dialer, index.clone(), events.clone()).await;
            }
            Err(e) => warn!("{addr} 연결 실패: {e}"),
        }
        sleep(backoff.next_delay()).await;
    }
}

async fn accept_loop(endpoint: &Endpoint, index: Arc<Vec<Entry>>, events: Sender<PeerEvent>) {
    loop {
        match rfsync_network::accept_quic(endpoint).await {
            Ok(conn) => {
                tokio::spawn(hold_connection(
                    conn,
                    SessionRole::Acceptor,
                    index.clone(),
                    events.clone(),
                ));
            }
            Err(NetworkError::QuicEndpointClosed) => return,
            Err(e) => warn!("연결 수락 실패: {e}"),
        }
    }
}

async fn hold_connection(
    conn: QuicConnection,
    role: SessionRole,
    index: Arc<Vec<Entry>>,
    events: Sender<PeerEvent>,
) {
    debug!("연결 시작: {role:?}, 보낼 파일 목록 {}개", index.len());

    // 핸드셰이크가 끝나도 스트림을 닫지 않고, 연결이 끝날 때까지 쥐고 있는다.
    let mut serving = None;
    let _stream = match handshake(&conn, role, &index).await {
        Ok((stream, peer)) => {
            if peer.version != PROTOCOL_VERSION {
                warn!(
                    "상대의 프로토콜 버전이 다릅니다: 상대 {}, 나 {PROTOCOL_VERSION}",
                    peer.version
                );
            }
            info!("상대 파일 목록 {}개를 받았습니다", peer.index.len());

            if events
                .send(PeerEvent::IndexReceived {
                    index: peer.index.clone(),
                })
                .await
                .is_err()
            {
                warn!("coordinator가 닫혀 있어 상대 목록을 전달하지 못하였습니다");
            }

            // 핸드셰이크가 끝난 연결에서만 상대의 파일 요청을 받는다.
            serving = Some(tokio::spawn(serve_requests(conn.clone(), events.clone())));
            Some(stream)
        }
        Err(e) => {
            warn!("핸드셰이크 실패: {e}");
            None
        }
    };

    let reason = conn.closed().await;
    info!("연결 종료: {reason}");
    if let Some(serving) = serving {
        serving.abort();
    }
}

/// 상대가 여는 스트림을 연결이 끝날 때까지 받는다. 스트림 하나는 파일 하나의 요청이다.
async fn serve_requests(conn: QuicConnection, events: Sender<PeerEvent>) {
    while let Ok((send, recv)) = accept_stream(&conn).await {
        let events = events.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_file_stream(send, recv, events).await {
                warn!("파일 요청 처리 실패: {e}");
            }
        });
    }
}

/// 파일 스트림 하나를 처리한다. `FileRequest`를 읽고 coordinator에게 파일을 받아,
/// 있으면 `FileFound` 헤더 뒤에 파일 내용을 프레임 없이 이어 보내고 없으면 `FileNotFound`를 보낸다.
async fn serve_file_stream(
    mut send: QuicSendStream,
    mut recv: QuicRecvStream,
    events: Sender<PeerEvent>,
) -> Result<(), EngineError> {
    let Message::FileRequest { path } = read_frame(&mut recv).await? else {
        return Err(EngineError::UnexpectedMessage("FileRequest"));
    };

    let (reply, answer) = oneshot::channel();
    let offer = if events
        .send(PeerEvent::FileRequested { path, reply })
        .await
        .is_ok()
    {
        // coordinator가 답하지 않고 끝났다면 없는 파일처럼 다룬다.
        answer.await.ok().flatten()
    } else {
        None
    };

    let Some(mut offer) = offer else {
        write_frame(&mut send, &Message::FileNotFound).await?;
        let _ = send.finish();
        return Ok(());
    };

    write_frame(
        &mut send,
        &Message::FileFound {
            size: offer.size,
            hash: offer.hash,
        },
    )
    .await?;

    // 파일이 그새 커졌어도 헤더에 적은 크기까지만 보낸다.
    let sent = tokio::io::copy(&mut (&mut offer.file).take(offer.size), &mut send).await?;
    if sent != offer.size {
        // 파일이 줄어들어 약속한 만큼 못 보냈다. 정상 종료로 보이지 않게 스트림을 끊는다.
        let _ = send.reset(0u32.into());
        warn!("약속한 {}바이트 중 {sent}바이트만 보냈습니다", offer.size);
        return Ok(());
    }
    let _ = send.finish();
    Ok(())
}

#[cfg(test)]
mod tests;
