//! `local`과 `peer`를 잇는 coordinator.

use std::path::Path;

use rfsync_protocol::Entry;
use tokio::sync::mpsc::{self, Receiver};
use tracing::{debug, info};

use crate::{
    error::EngineError,
    peer::{self, Identity, PeerEvent, load_index},
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
    let (events, receiver) = mpsc::channel(32);
    let coordinator = tokio::spawn(run_coordinator(index.clone(), receiver));

    let result = peer::run(identity, port, db_path, index, events, shutdown).await;

    // 종료 중에는 남은 이벤트를 기다리지 않는다. 세션이 끝난 뒤에도 연결 작업이 송신구를 쥐고 있을 수 있다.
    coordinator.abort();
    result
}

async fn run_coordinator(index: Vec<Entry>, mut events: Receiver<PeerEvent>) {
    while let Some(event) = events.recv().await {
        match event {
            PeerEvent::IndexReceived { index: peer_index } => {
                on_index_received(&index, &peer_index);
            }
        }
    }
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
}
