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
    local::{first_root_path, open_for_serving},
    peer::{self, FileOffer, Identity, PeerEvent, load_index},
    sync::compare::{IndexComparison, compare_index},
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
    let coordinator = tokio::spawn(run_coordinator(index.clone(), root, receiver));

    let result = peer::run(identity, port, db_path, index, events, shutdown).await;

    // 종료 중에는 남은 이벤트를 기다리지 않는다. 세션이 끝난 뒤에도 연결 작업이 송신구를 쥐고 있을 수 있다.
    coordinator.abort();
    result
}

async fn run_coordinator(
    index: Vec<Entry>,
    root: Option<PathBuf>,
    mut events: Receiver<PeerEvent>,
) {
    while let Some(event) = events.recv().await {
        match event {
            PeerEvent::IndexReceived {
                index: peer_index, ..
            } => {
                on_index_received(&index, &peer_index);
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
        tokio::spawn(run_coordinator(Vec::new(), root, receiver));

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
