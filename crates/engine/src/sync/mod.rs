//! `local`과 `peer`를 잇는 coordinator.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rfsync_protocol::{DeviceId, Entry};
use tokio::{
    fs::File,
    sync::{
        mpsc::{self, Receiver, Sender},
        watch,
    },
    task::spawn_blocking,
};
use tracing::{debug, info, warn};

use crate::{
    error::EngineError,
    local::{
        DeleteOutcome, PreserveOutcome, clear_pending, delete_if_unchanged, first_root_path,
        merge_versions, open_for_serving, preserve_file, record_pending, save_received_file,
        spawn_watcher,
    },
    peer::{self, FileOffer, Identity, PeerEvent, PeerHandle, load_index},
    sync::{
        compare::{IndexComparison, changed_entries, compare_index},
        conflict::{Winner, conflict_path, decide_winner},
    },
};

mod compare;
mod conflict;

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
                comparison.updated,
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

/// 상대가 지운 파일을 내 폴더에서 지운다. 내용이 목록의 해시와 같은 파일만 지우며, 한 파일이 실패해도 나머지는 계속한다.
///
/// 지운 뒤의 삭제 기록은 감시가 폴더 변화를 보고 DB에 남기고, 상대에게도 같은 경로로 알린다.
async fn delete_files(root: PathBuf, db_path: PathBuf, deleted: Vec<Entry>) {
    for entry in deleted {
        let (path, hash, root) = (entry.path.clone(), entry.hash, root.clone());
        let (pending_entry, pending_db) = (entry.clone(), db_path.clone());
        // 파일을 읽고 해시를 계산하는 일은 오래 걸릴 수 있어 블로킹 전용 스레드에서 한다.
        let result = spawn_blocking(move || {
            // 감시자가 삭제를 내가 지운 것으로 오해하지 않도록, 지우기 전에 상대가 지웠다는 기록을 남긴다.
            record_pending(&pending_db, &pending_entry, true)?;
            delete_if_unchanged(&root, &path, &hash)
        })
        .await;
        // 지우지 않았다면 기록은 쓸모가 없다. 남겨 두면 나중에 내가 고친 것으로 쓰이지 않고 상대의 버전이 쓰인다.
        if !matches!(result, Ok(Ok(DeleteOutcome::Deleted))) {
            clear_pending_logged(&db_path, &entry.path).await;
        }
        match result {
            Ok(Ok(DeleteOutcome::Deleted)) => {
                info!("상대가 지워서 파일을 지웠습니다: {}", entry.path)
            }
            Ok(Ok(DeleteOutcome::AlreadyGone)) => debug!("이미 없는 파일입니다: {}", entry.path),
            Ok(Ok(DeleteOutcome::Changed)) => {
                info!("내가 고친 파일이라 지우지 않습니다: {}", entry.path)
            }
            Ok(Ok(DeleteOutcome::NotRegularFile)) => {
                warn!("일반 파일이 아니라 지우지 않습니다: {}", entry.path)
            }
            Ok(Err(e)) => warn!("파일을 지우지 못했습니다: {}: {e}", entry.path),
            Err(e) => warn!("파일 지우기 작업이 실패했습니다: {}: {e}", entry.path),
        }
    }
}

/// 요청받은 파일을 열어 크기와 해시를 계산한다. 줄 수 없으면 이유를 로그에 남기고 `None`이다.
/// 상대에게는 이유를 알리지 않는다(없는 파일과 거절한 파일을 구분해 주지 않는다).
async fn serve_file(root: Option<PathBuf>, requested: String) -> Option<FileOffer> {
    let root = root?;
    let path = requested.clone();

    // 파일을 읽고 해시를 계산하는 일은 오래 걸릴 수 있어 블로킹 전용 스레드에서 한다.
    let served = match spawn_blocking(move || open_for_serving(&root, &requested)).await {
        Ok(Ok(Some(served))) => served,
        Ok(Ok(None)) => {
            debug!("요청받은 파일이 없습니다: {path:?}");
            return None;
        }
        Ok(Err(e)) => {
            warn!("요청받은 파일을 줄 수 없습니다: {path:?}: {e}");
            return None;
        }
        Err(e) => {
            warn!("파일 열기 작업이 실패했습니다: {path:?}: {e}");
            return None;
        }
    };

    Some(FileOffer {
        size: served.size,
        hash: served.hash,
        file: File::from_std(served.file),
    })
}

/// 내게 없는 파일(`missing`)과 상대가 더 최신인 파일(`updated`)을 상대에게서 하나씩 받아 온다.
/// `updated`는 내 파일을 덮어쓴다. 한 파일이 실패해도 나머지는 계속 받는다.
///
/// 파일마다 새 스트림을 쓰므로 동시에 받을 수도 있지만, 지금은 순서대로 하나씩 받는다.
async fn fetch_entries(
    peer: PeerHandle,
    root: PathBuf,
    db_path: PathBuf,
    missing: Vec<Entry>,
    updated: Vec<Entry>,
) {
    let targets = missing
        .iter()
        .map(|entry| (entry, false))
        .chain(updated.iter().map(|entry| (entry, true)));
    for (entry, overwrite) in targets {
        match fetch_file(&peer, &root, &db_path, entry, overwrite).await {
            Ok(FetchOutcome::Saved) => info!("파일을 받았습니다: {}", entry.path),
            Ok(FetchOutcome::MissingOnPeer) => {
                warn!("상대에게 파일이 없습니다: {}", entry.path)
            }
            Ok(FetchOutcome::ChangedOnPeer) => {
                warn!(
                    "목록을 받은 뒤 상대의 파일이 바뀌어 받지 않습니다: {}",
                    entry.path
                )
            }
            Err(e) => warn!("파일을 받지 못했습니다: {}: {e}", entry.path),
        }
    }
}

/// 충돌에서 내가 진 파일. 내 파일과 이긴 상대 파일의 짝이다.
struct LostConflict {
    mine: Entry,
    theirs: Entry,
}

/// 동시에 고친 파일 중 내가 진 것만 골라낸다. 이긴 쪽은 아무것도 하지 않는다. 진 쪽이 내용을 남기고
/// 이긴 쪽 파일을 받아 가면 양쪽이 같아진다. 상대 장치를 알 수 없으면 승자를 가릴 수 없어 건드리지 않는다.
fn lost_conflicts(
    mine: &[Entry],
    conflicts: &[Entry],
    me: DeviceId,
    peer_id: Option<DeviceId>,
) -> Vec<LostConflict> {
    let Some(peer_id) = peer_id else {
        if !conflicts.is_empty() {
            warn!("상대 장치를 알 수 없어 동시에 고친 파일을 처리하지 않습니다");
        }
        return Vec::new();
    };

    let mut lost = Vec::new();
    for theirs in conflicts {
        let Some(mine) = mine.iter().find(|entry| entry.path == theirs.path) else {
            continue;
        };
        match decide_winner(mine, theirs, me, peer_id) {
            Winner::Mine => info!(
                "동시에 고친 파일이 내 것이 이겨 유지합니다: {}",
                theirs.path
            ),
            Winner::Theirs => lost.push(LostConflict {
                mine: mine.clone(),
                theirs: theirs.clone(),
            }),
        }
    }
    lost
}

/// 내가 진 파일을 하나씩 처리한다. 내 내용을 `.sync-conflict-*` 이름으로 남긴 뒤, 이긴 상대 파일을 받아 덮어쓴다.
///
/// 남기는 데 실패하면 상대 파일로 덮어쓰지 않는다(내용을 잃지 않는 것이 먼저다). 받은 파일의 버전은
/// 두 쪽의 버전을 합친 것이라 양쪽을 모두 앞서, 이후 상대가 같은 내용으로 보고 버전만 합친다.
async fn resolve_conflicts(
    peer: PeerHandle,
    root: PathBuf,
    db_path: PathBuf,
    me: DeviceId,
    lost: Vec<LostConflict>,
) {
    for LostConflict { mine, theirs } in lost {
        let preserved = conflict_path(&mine.path, mine.modified_secs, me);
        let (blocking_root, path, name) = (root.clone(), mine.path.clone(), preserved.clone());
        let outcome =
            match spawn_blocking(move || preserve_file(&blocking_root, &path, &name)).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(e)) => {
                    warn!(
                        "진 파일을 남기지 못해 덮어쓰지 않습니다: {}: {e}",
                        mine.path
                    );
                    continue;
                }
                Err(e) => {
                    warn!("진 파일 남기기 작업이 실패했습니다: {}: {e}", mine.path);
                    continue;
                }
            };
        match outcome {
            PreserveOutcome::Preserved => {
                info!("동시에 고쳐 내가 져서 내 파일을 남겼습니다: {preserved}")
            }
            PreserveOutcome::SourceMissing => debug!("남길 파일이 이미 없습니다: {}", mine.path),
            PreserveOutcome::NotRegularFile | PreserveOutcome::AlreadyExists => {
                warn!(
                    "진 파일을 남길 수 없어 덮어쓰지 않습니다: {} ({outcome:?})",
                    mine.path
                );
                continue;
            }
        }

        let mut incoming = theirs;
        incoming.version.merge(&mine.version);
        match fetch_file(&peer, &root, &db_path, &incoming, true).await {
            Ok(FetchOutcome::Saved) => {
                info!("충돌에서 이긴 상대의 파일로 바꿨습니다: {}", incoming.path)
            }
            Ok(FetchOutcome::MissingOnPeer) => {
                warn!("상대에게 파일이 없습니다: {}", incoming.path)
            }
            Ok(FetchOutcome::ChangedOnPeer) => warn!(
                "목록을 받은 뒤 상대의 파일이 바뀌어 받지 않습니다: {}",
                incoming.path
            ),
            Err(e) => warn!(
                "충돌에서 이긴 파일을 받지 못했습니다: {}: {e}",
                incoming.path
            ),
        }
    }
}

/// 파일 하나를 받으려던 결과.
#[derive(Debug, PartialEq)]
enum FetchOutcome {
    Saved,
    /// 목록에는 있었는데 요청하니 상대가 없다고 답했다.
    MissingOnPeer,
    /// 상대가 알려 준 크기나 해시가 목록의 값과 달라, 목록을 받은 뒤 파일이 바뀐 것이다.
    ChangedOnPeer,
}

/// 파일 하나를 요청하고, 응답 헤더가 목록의 값과 같을 때만 내용을 받아 저장한다.
async fn fetch_file(
    peer: &PeerHandle,
    root: &Path,
    db_path: &Path,
    entry: &Entry,
    overwrite: bool,
) -> Result<FetchOutcome, EngineError> {
    let Some(mut download) = peer.request_file(&entry.path).await? else {
        return Ok(FetchOutcome::MissingOnPeer);
    };
    if !matches_entry(entry, download.size, &download.hash) {
        // 내용을 읽지 않고 스트림을 버린다. 상대는 전송이 중단됐다는 경고를 남긴다.
        return Ok(FetchOutcome::ChangedOnPeer);
    }

    // 감시자가 받은 파일을 내가 만든 것으로 오해하지 않도록, 파일이 생기기 전에 상대의 버전을 기록해 둔다.
    let (pending_db, pending_entry) = (db_path.to_path_buf(), entry.clone());
    spawn_blocking(move || record_pending(&pending_db, &pending_entry, false))
        .await
        .map_err(std::io::Error::other)??;

    let saved = save_received_file(
        root,
        &entry.path,
        download.size,
        download.hash,
        overwrite,
        &mut download.reader,
    )
    .await;
    if let Err(e) = saved {
        // 파일이 생기지 않았으므로 기록도 지운다.
        clear_pending_logged(db_path, &entry.path).await;
        return Err(e);
    }
    Ok(FetchOutcome::Saved)
}

/// 남은 `sync_pending` 기록을 지운다. 지우지 못해도 받는 일은 계속하므로 로그만 남긴다.
async fn clear_pending_logged(db_path: &Path, path: &str) {
    let (db_path, path) = (db_path.to_path_buf(), path.to_owned());
    let logged = path.clone();
    match spawn_blocking(move || clear_pending(&db_path, &path)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("남은 수신 기록을 지우지 못했습니다: {logged}: {e}"),
        Err(e) => warn!("수신 기록 지우기 작업이 실패했습니다: {logged}: {e}"),
    }
}

/// 응답 헤더의 크기와 해시가 상대가 보낸 목록의 항목과 같은가.
fn matches_entry(entry: &Entry, size: u64, hash: &[u8; 32]) -> bool {
    entry.size == size && entry.hash == *hash
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

    #[test]
    fn 응답_헤더가_목록의_크기와_해시와_같을_때만_일치한다() {
        let listed = entry("a.txt", 1); // 크기 1, 해시 [1; 32]

        assert!(matches_entry(&listed, 1, &[1; 32]));
        assert!(!matches_entry(&listed, 2, &[1; 32]), "크기가 다르다");
        assert!(!matches_entry(&listed, 1, &[2; 32]), "해시가 다르다");
    }

    #[tokio::test]
    async fn 루트가_없으면_none으로_답한다() {
        assert!(request(None, "a.txt").await.is_none());
    }
}
