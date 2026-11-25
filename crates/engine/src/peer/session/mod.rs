use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    pin::pin,
    sync::Arc,
};

use rfsync_network::{
    ClientConfig, Endpoint, NetworkError, QuicConnection, QuicRecvStream, QuicSendStream,
    accept_stream,
};
use rfsync_protocol::{Entry, Message, PROTOCOL_VERSION, read_frame, write_frame};
use tokio::{
    io::AsyncReadExt,
    sync::{
        mpsc::{self, Receiver, Sender},
        oneshot, watch,
    },
    task::JoinSet,
    time::sleep,
};
use tracing::{debug, info, warn};

use self::{backoff::Backoff, handshake::handshake};
use super::identity::{Identity, SERVER_NAME};
use crate::{
    error::EngineError,
    peer::{PeerEvent, PeerHandle},
};

mod backoff;
mod handshake;

/// 연결에서 이 쪽이 맡은 역할. 연결을 시작한 쪽이 스트림을 열고, 받은 쪽이 스트림을 받는다.
#[derive(Debug, Clone, Copy)]
enum SessionRole {
    Dialer,
    Acceptor,
}

/// 페어링된 상대들과 연결을 맺고 지킨다. 페어링된 상대가 없어도 시작하며, `devices_changed`로 알려 주면
/// 저장된 장치 목록을 다시 읽어 새 상대를 받아들이고 연결을 건다.
///
/// `index`는 연결이 새로 맺어질 때마다 그 시점의 값을 상대에게 보낸다.
pub async fn run(
    identity: &Identity,
    port: u16,
    db_path: &Path,
    index: watch::Receiver<Arc<Vec<Entry>>>,
    events: Sender<PeerEvent>,
    mut devices_changed: Receiver<()>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    let devices = list_devices(db_path)?;
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let endpoint = rfsync_network::bind_quic(addr, server_config_for(identity, &devices)?)?;

    let mut dialed = HashSet::new();
    let mut dials = JoinSet::new();
    start_dials(
        &endpoint,
        identity,
        &devices,
        &mut dialed,
        &mut dials,
        &index,
        &events,
    )?;
    let accept = tokio::spawn(accept_loop(endpoint.clone(), index.clone(), events.clone()));

    let mut shutdown = pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            // 알려 주는 쪽이 없어지면 이 분기만 꺼지고 나머지는 계속 돈다.
            Some(()) = devices_changed.recv() => {
                let result = list_devices(db_path).and_then(|devices| {
                    // 이미 맺은 연결은 그대로 두고, 앞으로 오는 접속부터 새 목록으로 검증한다.
                    endpoint.set_server_config(Some(server_config_for(identity, &devices)?));
                    start_dials(&endpoint, identity, &devices, &mut dialed, &mut dials, &index, &events)
                });
                match result {
                    Ok(()) => info!("페어링된 장치 목록을 다시 읽었습니다"),
                    Err(e) => warn!("페어링된 장치 목록을 반영하지 못했습니다: {e}"),
                }
            }
        }
    }

    accept.abort();
    endpoint.close(0u32.into(), b"shutdown");
    dials.shutdown().await;
    endpoint.wait_idle().await;

    Ok(())
}

fn list_devices(db_path: &Path) -> Result<Vec<rfsync_storage::Device>, EngineError> {
    let conn = rfsync_storage::open(db_path)?;
    Ok(rfsync_storage::list_devices(&conn)?)
}

/// 페어링된 장치의 인증서를 믿는 서버 설정. 장치가 없으면 내 인증서만 믿는다.
/// 내 인증서는 내 키로만 증명할 수 있으므로 아무도 접속하지 못하고, 빈 목록으로는 설정을 만들 수 없어서 쓴다.
fn server_config_for(
    identity: &Identity,
    devices: &[rfsync_storage::Device],
) -> Result<rfsync_network::ServerConfig, EngineError> {
    let mut certs: Vec<&[u8]> = devices.iter().map(|d| d.cert_der.as_slice()).collect();
    if certs.is_empty() {
        certs.push(&identity.cert);
    }
    Ok(rfsync_network::server_config(
        &identity.cert,
        &identity.key,
        &certs,
    )?)
}

/// 주소를 아는 장치 가운데 아직 연결을 걸지 않은 장치마다 연결 작업을 띄운다.
fn start_dials(
    endpoint: &Endpoint,
    identity: &Identity,
    devices: &[rfsync_storage::Device],
    dialed: &mut HashSet<Vec<u8>>,
    dials: &mut JoinSet<()>,
    index: &watch::Receiver<Arc<Vec<Entry>>>,
    events: &Sender<PeerEvent>,
) -> Result<(), EngineError> {
    for device in devices {
        if dialed.contains(&device.cert_der) {
            continue;
        }
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
        dialed.insert(device.cert_der.clone());
    }
    Ok(())
}

async fn dial_loop(
    endpoint: Endpoint,
    addr: SocketAddr,
    config: ClientConfig,
    index: watch::Receiver<Arc<Vec<Entry>>>,
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

async fn accept_loop(
    endpoint: Endpoint,
    index: watch::Receiver<Arc<Vec<Entry>>>,
    events: Sender<PeerEvent>,
) {
    loop {
        match rfsync_network::accept_quic(&endpoint).await {
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
    index: watch::Receiver<Arc<Vec<Entry>>>,
    events: Sender<PeerEvent>,
) {
    // 연결을 맺는 시점의 목록을 보낸다. 이후 변경은 `IndexUpdate`로 따로 알린다.
    let index = index.borrow().clone();
    debug!("연결 시작: {role:?}, 보낼 파일 목록 {}개", index.len());

    // 핸드셰이크 뒤에도 제어 스트림을 닫지 않고 연결이 끝날 때까지 쓴다. 작업은 JoinSet이 버려질 때 함께 끝난다.
    let mut tasks = JoinSet::new();
    // 이 송신구를 연결이 끝날 때까지 쥐고 있어, 손잡이를 모두 버려도 쓰기 작업이 끝나 스트림이 닫히지 않는다.
    let (updates, updates_rx) = mpsc::channel(32);
    match handshake(&conn, role, &index).await {
        Ok((stream, peer)) => {
            if peer.version != PROTOCOL_VERSION {
                warn!(
                    "상대의 프로토콜 버전이 다릅니다: 상대 {}, 나 {PROTOCOL_VERSION}",
                    peer.version
                );
            }
            info!("상대 파일 목록 {}개를 받았습니다", peer.index.len());

            let handle = PeerHandle::new(conn.clone(), updates.clone());
            if events
                .send(PeerEvent::IndexReceived {
                    index: peer.index.clone(),
                    peer: handle.clone(),
                })
                .await
                .is_err()
            {
                warn!("coordinator가 닫혀 있어 상대 목록을 전달하지 못하였습니다");
            }

            // 핸드셰이크가 끝난 연결에서만 상대의 파일 요청을 받는다.
            tasks.spawn(serve_requests(conn.clone(), events.clone()));
            // 읽기와 쓰기를 따로 돌린다. 프레임 읽기는 select!로 중간에 끊으면 안 되기 때문이다.
            tasks.spawn(read_updates(stream.recv, handle, events.clone()));
            tasks.spawn(write_updates(stream.send, updates_rx));
        }
        Err(e) => warn!("핸드셰이크 실패: {e}"),
    }

    let reason = conn.closed().await;
    info!("연결 종료: {reason}");
    tasks.shutdown().await;
    drop(updates);
}

/// 상대가 제어 스트림으로 보내는 `IndexUpdate`를 연결이 끝날 때까지 coordinator에게 전달한다.
async fn read_updates(mut recv: QuicRecvStream, peer: PeerHandle, events: Sender<PeerEvent>) {
    loop {
        match read_frame(&mut recv).await {
            Ok(Message::IndexUpdate { entries }) => {
                if events
                    .send(PeerEvent::IndexUpdated {
                        entries,
                        peer: peer.clone(),
                    })
                    .await
                    .is_err()
                {
                    warn!("coordinator가 닫혀 있어 상대의 변경을 전달하지 못하였습니다");
                    return;
                }
            }
            Ok(_) => {
                warn!("제어 스트림에 예상하지 못한 메시지가 왔습니다");
                return;
            }
            // 연결이 끝나면 읽기가 실패한다. 정상적인 종료다.
            Err(e) => {
                debug!("제어 스트림 읽기 종료: {e}");
                return;
            }
        }
    }
}

/// 손잡이로 들어온 변경 항목을 제어 스트림으로 상대에게 보낸다.
async fn write_updates(mut send: QuicSendStream, mut updates: Receiver<Vec<Entry>>) {
    while let Some(entries) = updates.recv().await {
        if let Err(e) = write_frame(&mut send, &Message::IndexUpdate { entries }).await {
            warn!("변경 목록을 보내지 못했습니다: {e}");
            return;
        }
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
