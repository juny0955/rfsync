use std::{path::PathBuf, sync::Arc};

use rfsync_protocol::{DeviceId, Entry};
use tokio::{
    sync::{
        mpsc::{Receiver, Sender},
        watch,
    },
    task::spawn_blocking,
};
use tracing::{debug, info, warn};

use crate::{
    error::EngineError,
    local::{first_root_path, merge_versions, spawn_watcher},
    peer::{PeerEvent, PeerHandle, load_index},
    sync::{compare::changed_entries, plan::SyncPlan, serve::serve_file},
};

/// 세션 이벤트와 내 폴더의 변화를 받아 내 파일 목록을 상대와 맞추는 상태.
pub(super) struct Coordinator {
    pub me: DeviceId,
    pub index: Vec<Entry>,
    pub root: Option<PathBuf>,
    pub db_path: PathBuf,
    /// 연결된 상대들. 내 폴더가 바뀌면 이들에게 알린다.
    pub peers: Vec<PeerHandle>,
    /// 바뀐 내 파일 목록을 세션에 알린다.
    pub index_tx: watch::Sender<Arc<Vec<Entry>>>,
    /// 감시를 새로 띄울 때 넘겨 줄 "내 폴더가 바뀌었다" 신호의 송신구.
    pub local_signal: Sender<()>,
}

/// coordinator가 기다리는 신호들.
pub(super) struct Inbox {
    pub events: Receiver<PeerEvent>,
    pub local_changed: Receiver<()>,
    /// CLI가 루트를 등록했다는 알림.
    pub root_added: Receiver<()>,
}

impl Coordinator {
    pub(super) async fn run(mut self, mut inbox: Inbox) {
        loop {
            tokio::select! {
                event = inbox.events.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        PeerEvent::IndexReceived { index, peer } => {
                            self.peers.push(peer.clone());
                            self.on_peer_entries(peer, &index).await;
                        }
                        // 연결된 뒤의 변경도 첫 목록과 같은 비교와 받아 오기를 거친다. 바뀐 항목만 들어 있다.
                        PeerEvent::IndexUpdated { entries, peer } => {
                            self.on_peer_entries(peer, &entries).await;
                        }
                        PeerEvent::FileRequested { path, reply } => {
                            // 큰 파일의 해시 계산이 다른 이벤트 처리를 막지 않도록 요청마다 따로 처리한다.
                            let root = self.root.clone();
                            tokio::spawn(async move {
                                let offer = serve_file(root, path).await;
                                // 요청한 세션이 이미 끝났으면 받는 쪽이 없다. 버려도 된다.
                                let _ = reply.send(offer);
                            });
                        }
                    }
                }
                // 감시가 끝나 송신구가 닫히면 이 분기만 꺼지고 나머지는 계속 돈다.
                Some(()) = inbox.local_changed.recv() => self.on_local_changed().await,
                Some(()) = inbox.root_added.recv() => self.on_root_added().await,
            }
        }
    }

    /// 상대의 목록(또는 변경 목록)을 내 목록과 비교해, 받아 오기·지우기·충돌 해결 작업을 띄우고
    /// 내용이 같아 버전만 합치면 되는 항목은 바로 합친다.
    async fn on_peer_entries(&mut self, peer: PeerHandle, theirs: &[Entry]) {
        let SyncPlan { transfers, merge } =
            SyncPlan::new(&self.index, theirs, self.me, peer.device_id());
        match &self.root {
            Some(root) => transfers.spawn(peer, root, &self.db_path, self.me),
            None => debug!("등록된 루트가 없어 파일을 받거나 지우지 않습니다"),
        }
        self.merge_and_reload(merge).await;
    }

    /// 데몬이 도는 중에 루트가 등록되었다.
    /// 임시: 루트가 하나뿐이라고 가정해, 처음 생긴 루트만 감시하고 이후에 추가된 루트는 다음 시작 때 반영한다.
    async fn on_root_added(&mut self) {
        if self.root.is_some() {
            warn!("이미 루트가 있어 새 루트는 데몬을 다시 시작해야 반영됩니다");
            return;
        }
        match first_root_path(&self.db_path) {
            Ok(Some(path)) => {
                match spawn_watcher(&self.db_path, self.local_signal.clone(), self.me) {
                    Ok(()) => {
                        info!("새 루트를 감시합니다: {}", path.display());
                        self.root = Some(path);
                        self.on_local_changed().await;
                    }
                    Err(e) => warn!("새 루트의 감시를 시작하지 못했습니다: {e}"),
                }
            }
            Ok(None) => {}
            Err(e) => warn!("새 루트를 읽지 못했습니다: {e}"),
        }
    }

    /// 내 폴더가 바뀌었다. DB에서 목록을 다시 읽어 직전 목록과 비교하고, 바뀐 항목을 연결된 상대 모두에게 보낸다.
    async fn on_local_changed(&mut self) {
        let db_path = self.db_path.clone();
        let new_index = match spawn_blocking(move || load_index(&db_path)).await {
            Ok(Ok(new_index)) => new_index,
            Ok(Err(e)) => return warn!("바뀐 파일 목록을 읽지 못했습니다: {e}"),
            Err(e) => return warn!("파일 목록 읽기 작업이 실패했습니다: {e}"),
        };

        let changed = changed_entries(&self.index, &new_index);
        self.index = new_index;
        self.index_tx.send_replace(Arc::new(self.index.clone()));
        if changed.is_empty() {
            return;
        }
        info!("내 파일 {}개가 바뀌어 상대에게 알립니다", changed.len());

        // 보내지 못한 상대는 연결이 끝난 것이므로 목록에서 뺀다.
        let mut alive = Vec::with_capacity(self.peers.len());
        for peer in self.peers.drain(..) {
            if peer.send_update(changed.clone()).await.is_ok() {
                alive.push(peer);
            }
        }
        self.peers = alive;
    }

    /// 내용이 같은 파일의 버전을 합쳐 DB에 저장하고, 바뀌었으면 목록을 다시 읽어 세션에 알린다.
    /// 상대에게는 알리지 않는다. 내용이 그대로여서 상대가 알아야 할 변경이 아니고, 상대도 같은 비교로 스스로 합친다.
    async fn merge_and_reload(&mut self, merge: Vec<Entry>) {
        if merge.is_empty() {
            return;
        }
        let path = self.db_path.clone();
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
                self.index = new_index;
                self.index_tx.send_replace(Arc::new(self.index.clone()));
            }
            Ok(Ok((_, None))) => {}
            Ok(Err(e)) => warn!("파일 버전을 합치지 못했습니다: {e}"),
            Err(e) => warn!("파일 버전 합치기 작업이 실패했습니다: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::peer::FileOffer;

    /// coordinator를 띄우고, 파일 하나를 요청해 답을 받아 온다.
    async fn request(root: Option<PathBuf>, path: &str) -> Option<FileOffer> {
        let (events, receiver) = mpsc::channel(8);
        let (local_signal, local_changed) = mpsc::channel(1);
        let (_root_added, root_added) = mpsc::channel(1);
        let (index_tx, _index_rx) = watch::channel(Arc::new(Vec::new()));
        let coordinator = Coordinator {
            me: DeviceId::from_cert(b"test"),
            index: Vec::new(),
            root,
            db_path: PathBuf::new(),
            peers: Vec::new(),
            index_tx,
            local_signal,
        };
        tokio::spawn(coordinator.run(Inbox {
            events: receiver,
            local_changed,
            root_added,
        }));

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
