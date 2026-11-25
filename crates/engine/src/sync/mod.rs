//! `local`과 `peer`를 잇는 coordinator.

use std::{path::Path, sync::Arc};

use tokio::sync::{mpsc, watch};

use crate::{
    error::EngineError,
    local::{first_root_path, spawn_watcher},
    peer::{self, Identity, load_index},
    sync::coordinator::{Coordinator, Inbox},
};

mod compare;
mod conflict;
mod coordinator;
mod delete;
mod fetch;
mod plan;
mod resolve;
mod serve;

/// 데몬 본체. 내 파일 목록을 읽고, 세션이 보내는 이벤트를 처리하는 coordinator를 띄운 뒤 세션을 실행한다.
pub async fn run(
    identity: &Identity,
    port: u16,
    db_path: &Path,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    // CLI가 루트를 등록하거나 장치를 페어링했다는 알림. 용량 1: 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽는다.
    let (root_added, root_added_rx) = mpsc::channel(1);
    let (devices_changed, devices_changed_rx) = mpsc::channel(1);
    // CLI 요청을 받는 소켓. 이미 데몬이 떠 있으면 여기서 바로 실패한다. 함수가 끝나면 소켓 파일을 지운다.
    #[cfg(unix)]
    let _ipc = crate::ipc::start(
        &crate::socket_path(db_path),
        crate::ipc::IpcContext {
            db_path: db_path.to_path_buf(),
            port,
            cert: identity.cert.clone(),
            root_added,
            devices_changed,
        },
    )?;
    #[cfg(not(unix))]
    drop((root_added, devices_changed));
    let index = load_index(db_path)?;
    let root = first_root_path(db_path)?;
    let (events, events_rx) = mpsc::channel(32);
    // 용량 1: 신호가 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽으므로 더 쌓을 필요가 없다.
    let (local_signal, local_changed) = mpsc::channel(1);
    spawn_watcher(db_path, local_signal.clone(), identity.device_id())?;
    // 세션은 연결을 맺을 때마다 이 목록의 최신 값을 상대에게 보낸다.
    let (index_tx, index_rx) = watch::channel(Arc::new(index.clone()));
    let coordinator = Coordinator {
        me: identity.device_id(),
        index,
        root,
        db_path: db_path.to_path_buf(),
        peers: Vec::new(),
        index_tx,
        local_signal,
    };
    let coordinator = tokio::spawn(coordinator.run(Inbox {
        events: events_rx,
        local_changed,
        root_added: root_added_rx,
    }));

    let result = peer::run(
        identity,
        port,
        db_path,
        index_rx,
        events,
        devices_changed_rx,
        shutdown,
    )
    .await;

    // 종료 중에는 남은 이벤트를 기다리지 않는다. 세션이 끝난 뒤에도 연결 작업이 송신구를 쥐고 있을 수 있다.
    coordinator.abort();
    result
}

/// 파일을 읽거나 DB를 여는 블로킹 작업을 전용 스레드에서 돌린다.
/// 작업이 패닉 등으로 끝나지 못한 경우도 작업의 오류와 같은 `EngineError`로 돌려준다.
async fn blocking<T, F>(work: F) -> Result<T, EngineError>
where
    F: FnOnce() -> Result<T, EngineError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(std::io::Error::other)?
}
