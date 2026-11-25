use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rfsync_protocol::{DeviceId, Entry};
use tokio::{
    sync::{
        mpsc::{self, Receiver, Sender},
        watch,
    },
    task::spawn_blocking,
};
use tracing::{debug, info, warn};

use crate::{
    error::EngineError,
    local::{first_root_path, merge_versions, spawn_watcher},
    peer::{self, Identity, PeerEvent, PeerHandle, load_index},
    sync::{
        compare::{IndexComparison, changed_entries, compare_index},
        delete::delete_files,
        fetch::{fetch_entries, with_local_hashes},
        resolve::{lost_conflicts, resolve_conflicts},
        serve::serve_file,
    },
};

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
    let (events, receiver) = mpsc::channel(32);
    // 용량 1: 신호가 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽으므로 더 쌓을 필요가 없다.
    let (local_changed, local_receiver) = mpsc::channel(1);
    spawn_watcher(db_path, local_changed.clone(), identity.device_id())?;
    // 세션은 연결을 맺을 때마다 이 목록의 최신 값을 상대에게 보낸다.
    let (index_tx, index_rx) = watch::channel(Arc::new(index.clone()));
    let coordinator = tokio::spawn(run_coordinator(
        index,
        root,
        db_path.to_path_buf(),
        receiver,
        local_receiver,
        root_added_rx,
        Wiring {
            me: identity.device_id(),
            index_tx,
            local_signal: local_changed,
        },
    ));

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

/// coordinator가 바깥과 주고받는 연결부.
struct Wiring {
    me: DeviceId,
    /// 바뀐 내 파일 목록을 세션에 알린다.
    index_tx: watch::Sender<Arc<Vec<Entry>>>,
    /// 감시를 새로 띄울 때 넘겨 줄 "내 폴더가 바뀌었다" 신호의 송신구.
    local_signal: Sender<()>,
}

async fn run_coordinator(
    mut index: Vec<Entry>,
    mut root: Option<PathBuf>,
    db_path: PathBuf,
    mut events: Receiver<PeerEvent>,
    mut local_changed: Receiver<()>,
    mut root_added: Receiver<()>,
    wiring: Wiring,
) {
    // 연결된 상대들. 내 폴더가 바뀌면 이들에게 알린다.
    let mut peers: Vec<PeerHandle> = Vec::new();

    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    PeerEvent::IndexReceived {
                        index: peer_index,
                        peer,
                    } => {
                        peers.push(peer.clone());
                        let merge = start_fetch(&index, &root, &db_path, wiring.me, peer, &peer_index);
                        merge_and_reload(&mut index, &db_path, merge, &wiring.index_tx).await;
                    }
                    // 연결된 뒤의 변경도 첫 목록과 같은 비교와 받아 오기를 거친다. 바뀐 항목만 들어 있다.
                    PeerEvent::IndexUpdated { entries, peer } => {
                        let merge = start_fetch(&index, &root, &db_path, wiring.me, peer, &entries);
                        merge_and_reload(&mut index, &db_path, merge, &wiring.index_tx).await;
                    }
                    PeerEvent::FileRequested { path, reply } => {
                        // 큰 파일의 해시 계산이 다른 이벤트 처리를 막지 않도록 요청마다 따로 처리한다.
                        let root = root.clone();
                        tokio::spawn(async move {
                            let offer = serve_file(root, path).await;
                            // 요청한 세션이 이미 끝났으면 받는 쪽이 없다. 버려도 된다.
                            let _ = reply.send(offer);
                        });
                    }
                }
            }
            // 감시가 끝나 송신구가 닫히면 이 분기만 꺼지고 나머지는 계속 돈다.
            Some(()) = local_changed.recv() => {
                on_local_changed(&mut index, &mut peers, &db_path, &wiring.index_tx).await;
            }
            // 데몬이 도는 중에 루트가 등록되었다.
            // 임시: 루트가 하나뿐이라고 가정해, 처음 생긴 루트만 감시하고 이후에 추가된 루트는 다음 시작 때 반영한다.
            Some(()) = root_added.recv() => {
                if root.is_some() {
                    warn!("이미 루트가 있어 새 루트는 데몬을 다시 시작해야 반영됩니다");
                    continue;
                }
                match first_root_path(&db_path) {
                    Ok(Some(path)) => {
                        match spawn_watcher(&db_path, wiring.local_signal.clone(), wiring.me) {
                            Ok(()) => {
                                info!("새 루트를 감시합니다: {}", path.display());
                                root = Some(path);
                                on_local_changed(&mut index, &mut peers, &db_path, &wiring.index_tx).await;
                            }
                            Err(e) => warn!("새 루트의 감시를 시작하지 못했습니다: {e}"),
                        }
                    }
                    Ok(None) => {}
                    Err(e) => warn!("새 루트를 읽지 못했습니다: {e}"),
                }
            }
        }
    }
}

/// 내 폴더가 바뀌었다. DB에서 목록을 다시 읽어 직전 목록과 비교하고, 바뀐 항목을 연결된 상대 모두에게 보낸다.
async fn on_local_changed(
    index: &mut Vec<Entry>,
    peers: &mut Vec<PeerHandle>,
    db_path: &Path,
    index_tx: &watch::Sender<Arc<Vec<Entry>>>,
) {
    let db_path = db_path.to_path_buf();
    let new_index = match spawn_blocking(move || load_index(&db_path)).await {
        Ok(Ok(new_index)) => new_index,
        Ok(Err(e)) => return warn!("바뀐 파일 목록을 읽지 못했습니다: {e}"),
        Err(e) => return warn!("파일 목록 읽기 작업이 실패했습니다: {e}"),
    };

    let changed = changed_entries(index, &new_index);
    *index = new_index;
    index_tx.send_replace(Arc::new(index.clone()));
    if changed.is_empty() {
        return;
    }
    info!("내 파일 {}개가 바뀌어 상대에게 알립니다", changed.len());

    // 보내지 못한 상대는 연결이 끝난 것이므로 목록에서 뺀다.
    let mut alive = Vec::with_capacity(peers.len());
    for peer in peers.drain(..) {
        if peer.send_update(changed.clone()).await.is_ok() {
            alive.push(peer);
        }
    }
    *peers = alive;
}

/// 상대의 목록(또는 변경 목록)을 내 목록과 비교해, 내게 없거나 상대가 더 최신인 파일을 받아 오고
/// 상대가 지운 파일을 지우고, 동시에 고친 파일은 진 쪽을 남기고 이긴 쪽으로 맞추는 작업을 띄운다.
/// 내용이 같아 버전만 합치면 되는 항목을 돌려준다.
fn start_fetch(
    mine: &[Entry],
    root: &Option<PathBuf>,
    db_path: &Path,
    me: DeviceId,
    peer: PeerHandle,
    theirs: &[Entry],
) -> Vec<Entry> {
    let comparison = on_index_received(mine, theirs);
    let lost = lost_conflicts(mine, &comparison.conflicts, me, peer.device_id());
    match root.clone() {
        // 연결마다 따로 받는다. 한 연결에서 받는 중에도 다른 이벤트 처리가 막히지 않는다.
        Some(root) => {
            if !comparison.deleted.is_empty() {
                tokio::spawn(delete_files(
                    root.clone(),
                    db_path.to_path_buf(),
                    comparison.deleted,
                ));
            }
            if !lost.is_empty() {
                tokio::spawn(resolve_conflicts(
                    peer.clone(),
                    root.clone(),
                    db_path.to_path_buf(),
                    me,
                    lost,
                ));
            }
            tokio::spawn(fetch_entries(
                peer,
                root,
                db_path.to_path_buf(),
                comparison.missing,
                with_local_hashes(mine, comparison.updated),
            ));
        }
        None => debug!("등록된 루트가 없어 파일을 받거나 지우지 않습니다"),
    }
    comparison.merge
}

/// 내용이 같은 파일의 버전을 합쳐 DB에 저장하고, 바뀌었으면 목록을 다시 읽어 세션에 알린다.
/// 상대에게는 알리지 않는다. 내용이 그대로여서 상대가 알아야 할 변경이 아니고, 상대도 같은 비교로 스스로 합친다.
async fn merge_and_reload(
    index: &mut Vec<Entry>,
    db_path: &Path,
    merge: Vec<Entry>,
    index_tx: &watch::Sender<Arc<Vec<Entry>>>,
) {
    if merge.is_empty() {
        return;
    }
    let path = db_path.to_path_buf();
    let result = spawn_blocking(move || {
        let merged = merge_versions(&path, &merge)?;
        let index = if merged > 0 {
            Some(load_index(&path)?)
        } else {
            None
        };
        Ok::<_, EngineError>((merged, index))
    })
    .await;
    match result {
        Ok(Ok((merged, Some(new_index)))) => {
            info!("내용이 같은 파일 {merged}개의 버전을 합쳤습니다");
            *index = new_index;
            index_tx.send_replace(Arc::new(index.clone()));
        }
        Ok(Ok((_, None))) => {}
        Ok(Err(e)) => warn!("파일 버전을 합치지 못했습니다: {e}"),
        Err(e) => warn!("파일 버전 합치기 작업이 실패했습니다: {e}"),
    }
}

fn on_index_received(mine: &[Entry], theirs: &[Entry]) -> IndexComparison {
    let comparison = compare_index(mine, theirs);
    info!(
        "내게 없는 파일 {}개, 상대가 더 최신인 파일 {}개, 동시에 고친 파일 {}개, 상대가 지운 파일 {}개, 버전만 합칠 파일 {}개",
        comparison.missing.len(),
        comparison.updated.len(),
        comparison.conflicts.len(),
        comparison.deleted.len(),
        comparison.merge.len()
    );

    for entry in &comparison.missing {
        debug!("내게 없는 파일: {}", entry.path);
    }

    for entry in &comparison.updated {
        debug!("상대가 더 최신인 파일: {}", entry.path);
    }

    for entry in &comparison.conflicts {
        warn!("양쪽에서 동시에 고친 파일: {}", entry.path);
    }

    for entry in &comparison.deleted {
        debug!("상대가 지운 파일: {}", entry.path);
    }

    comparison
}

#[cfg(test)]
mod tests {
    use rfsync_protocol::VersionVector;

    use super::*;
    use crate::peer::FileOffer;

    fn entry(path: &str, hash: u8) -> Entry {
        Entry {
            path: path.to_owned(),
            size: 1,
            hash: [hash; 32],
            modified_secs: 100,
            modified_nanos: 0,
            deleted: false,
            version: VersionVector::default(),
        }
    }

    #[test]
    fn 상대에게만_있는_파일과_상대가_더_최신인_파일을_나눠서_돌려준다() {
        let mut newer = entry("b.txt", 9);
        newer.version.bump(DeviceId::from_cert(b"peer"));
        let mine = vec![entry("a.txt", 1), entry("b.txt", 2)];
        let theirs = vec![entry("a.txt", 1), newer.clone(), entry("c.txt", 3)];

        let comparison = on_index_received(&mine, &theirs);

        assert_eq!(comparison.missing, vec![entry("c.txt", 3)]);
        assert_eq!(comparison.updated, vec![newer]);
    }

    /// coordinator를 띄우고, 파일 하나를 요청해 답을 받아 온다.
    async fn request(root: Option<PathBuf>, path: &str) -> Option<FileOffer> {
        let (events, receiver) = mpsc::channel(8);
        let (_local_changed, local_receiver) = mpsc::channel(1);
        let (_root_added, root_added_rx) = mpsc::channel(1);
        let (index_tx, _index_rx) = watch::channel(Arc::new(Vec::new()));
        tokio::spawn(run_coordinator(
            Vec::new(),
            root,
            PathBuf::new(),
            receiver,
            local_receiver,
            root_added_rx,
            Wiring {
                me: DeviceId::from_cert(b"test"),
                index_tx,
                local_signal: _local_changed,
            },
        ));

        let (reply, answer) = tokio::sync::oneshot::channel();
        events
            .send(PeerEvent::FileRequested {
                path: path.to_owned(),
                reply,
            })
            .await
            .unwrap();
        answer.await.unwrap()
    }

    #[tokio::test]
    async fn 요청한_파일의_크기와_해시와_내용을_돌려준다() {
        use tokio::io::AsyncReadExt;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("sub/a.txt"), "hello").unwrap();

        let mut offer = request(Some(root.path().to_owned()), "sub/a.txt")
            .await
            .unwrap();

        assert_eq!(offer.size, 5);
        assert_eq!(offer.hash, *blake3::hash(b"hello").as_bytes());
        let mut content = String::new();
        offer.file.read_to_string(&mut content).await.unwrap();
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn 없는_파일과_규칙을_어긴_경로는_none으로_답한다() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "x").unwrap();
        let root = Some(root.path().to_owned());

        assert!(request(root.clone(), "missing.txt").await.is_none());
        assert!(request(root.clone(), "../a.txt").await.is_none());
        assert!(request(root, "/etc/passwd").await.is_none());
    }

    #[tokio::test]
    async fn 루트가_없으면_none으로_답한다() {
        assert!(request(None, "a.txt").await.is_none());
    }
}
