//! `local`과 `peer`를 잇는 coordinator.

use std::path::{Path, PathBuf};

use rfsync_protocol::Entry;
use tokio::{
    fs::File,
    sync::mpsc::{self, Receiver},
    task::spawn_blocking,
};
use tracing::{debug, info, warn};

use crate::{
    error::EngineError,
    local::{first_root_path, open_for_serving, save_received_file, spawn_watcher},
    peer::{self, FileOffer, Identity, PeerEvent, PeerHandle, load_index},
    sync::compare::{IndexComparison, changed_entries, compare_index},
};

mod compare;

/// 데몬 본체. 내 파일 목록을 읽고, 세션이 보내는 이벤트를 처리하는 coordinator를 띄운 뒤 세션을 실행한다.
pub async fn run(
    identity: &Identity,
    port: u16,
    db_path: &Path,
    shutdown: impl Future<Output = ()>,
) -> Result<(), EngineError> {
    let index = load_index(db_path)?;
    let root = first_root_path(db_path)?;
    let (events, receiver) = mpsc::channel(32);
    // 용량 1: 신호가 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽으므로 더 쌓을 필요가 없다.
    let (local_changed, local_receiver) = mpsc::channel(1);
    spawn_watcher(db_path, local_changed)?;
    let coordinator = tokio::spawn(run_coordinator(
        index.clone(),
        root,
        db_path.to_path_buf(),
        receiver,
        local_receiver,
    ));

    let result = peer::run(identity, port, db_path, index, events, shutdown).await;

    // 종료 중에는 남은 이벤트를 기다리지 않는다. 세션이 끝난 뒤에도 연결 작업이 송신구를 쥐고 있을 수 있다.
    coordinator.abort();
    result
}

async fn run_coordinator(
    mut index: Vec<Entry>,
    root: Option<PathBuf>,
    db_path: PathBuf,
    mut events: Receiver<PeerEvent>,
    mut local_changed: Receiver<()>,
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
                        start_fetch(&index, &root, peer, &peer_index);
                    }
                    // 연결된 뒤의 변경도 첫 목록과 같은 비교와 받아 오기를 거친다. 바뀐 항목만 들어 있다.
                    PeerEvent::IndexUpdated { entries, peer } => {
                        start_fetch(&index, &root, peer, &entries)
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
                on_local_changed(&mut index, &mut peers, &db_path).await;
            }
        }
    }
}

/// 내 폴더가 바뀌었다. DB에서 목록을 다시 읽어 직전 목록과 비교하고, 바뀐 항목을 연결된 상대 모두에게 보낸다.
async fn on_local_changed(index: &mut Vec<Entry>, peers: &mut Vec<PeerHandle>, db_path: &Path) {
    let db_path = db_path.to_path_buf();
    let new_index = match spawn_blocking(move || load_index(&db_path)).await {
        Ok(Ok(new_index)) => new_index,
        Ok(Err(e)) => return warn!("바뀐 파일 목록을 읽지 못했습니다: {e}"),
        Err(e) => return warn!("파일 목록 읽기 작업이 실패했습니다: {e}"),
    };

    let changed = changed_entries(index, &new_index);
    *index = new_index;
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

/// 상대의 목록(또는 변경 목록)을 내 목록과 비교해, 내게 없는 파일을 받아 오는 작업을 띄운다.
fn start_fetch(mine: &[Entry], root: &Option<PathBuf>, peer: PeerHandle, theirs: &[Entry]) {
    let comparison = on_index_received(mine, theirs);
    match root.clone() {
        // 연결마다 따로 받는다. 한 연결에서 받는 중에도 다른 이벤트 처리가 막히지 않는다.
        Some(root) => {
            tokio::spawn(fetch_missing(peer, root, comparison.missing));
        }
        None => debug!("등록된 루트가 없어 파일을 받지 않습니다"),
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

/// 내게 없는 파일을 상대에게서 하나씩 받아 온다. 한 파일이 실패해도 나머지는 계속 받는다.
///
/// 파일마다 새 스트림을 쓰므로 동시에 받을 수도 있지만, 지금은 순서대로 하나씩 받는다.
async fn fetch_missing(peer: PeerHandle, root: PathBuf, missing: Vec<Entry>) {
    for entry in &missing {
        match fetch_file(&peer, &root, entry).await {
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
    entry: &Entry,
) -> Result<FetchOutcome, EngineError> {
    let Some(mut download) = peer.request_file(&entry.path).await? else {
        return Ok(FetchOutcome::MissingOnPeer);
    };
    if !matches_entry(entry, download.size, &download.hash) {
        // 내용을 읽지 않고 스트림을 버린다. 상대는 전송이 중단됐다는 경고를 남긴다.
        return Ok(FetchOutcome::ChangedOnPeer);
    }

    save_received_file(
        root,
        &entry.path,
        download.size,
        download.hash,
        &mut download.reader,
    )
    .await?;
    Ok(FetchOutcome::Saved)
}

/// 응답 헤더의 크기와 해시가 상대가 보낸 목록의 항목과 같은가.
fn matches_entry(entry: &Entry, size: u64, hash: &[u8; 32]) -> bool {
    entry.size == size && entry.hash == *hash
}

fn on_index_received(mine: &[Entry], theirs: &[Entry]) -> IndexComparison {
    let comparison = compare_index(mine, theirs);
    info!(
        "내게 없는 파일 {}개, 내용이 다른 파일 {}개",
        comparison.missing.len(),
        comparison.changed.len()
    );

    for entry in &comparison.missing {
        debug!("내게 없는 파일: {}", entry.path);
    }

    for entry in &comparison.changed {
        debug!("내용이 다른 파일: {}", entry.path);
    }

    comparison
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, hash: u8) -> Entry {
        Entry {
            path: path.to_owned(),
            size: 1,
            hash: [hash; 32],
            modified_secs: 100,
            modified_nanos: 0,
        }
    }

    #[test]
    fn 상대에게만_있는_파일과_내용이_다른_파일을_나눠서_돌려준다() {
        let mine = vec![entry("a.txt", 1), entry("b.txt", 2)];
        let theirs = vec![entry("a.txt", 1), entry("b.txt", 9), entry("c.txt", 3)];

        let comparison = on_index_received(&mine, &theirs);

        assert_eq!(comparison.missing, vec![entry("c.txt", 3)]);
        assert_eq!(comparison.changed, vec![entry("b.txt", 9)]);
    }

    /// coordinator를 띄우고, 파일 하나를 요청해 답을 받아 온다.
    async fn request(root: Option<PathBuf>, path: &str) -> Option<FileOffer> {
        let (events, receiver) = mpsc::channel(8);
        let (_local_changed, local_receiver) = mpsc::channel(1);
        tokio::spawn(run_coordinator(
            Vec::new(),
            root,
            PathBuf::new(),
            receiver,
            local_receiver,
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
