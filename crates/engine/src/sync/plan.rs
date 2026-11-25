use std::path::Path;

use rfsync_protocol::{DeviceId, Entry};
use tracing::{debug, info, warn};

use crate::{
    peer::PeerHandle,
    sync::{
        compare::compare_index,
        delete::delete_files,
        fetch::fetch_entries,
        resolve::{LostConflict, lost_conflicts, resolve_conflicts},
    },
};

/// 상대의 목록(또는 변경 목록)을 내 목록과 비교해 정한 할 일.
pub(super) struct SyncPlan {
    /// 상대에게서 파일을 받거나 내 파일을 지우는, 오래 걸리는 작업.
    pub transfers: Transfers,
    /// 내용이 같아 버전만 합치면 되는 항목.
    pub merge: Vec<Entry>,
}

pub(super) struct Transfers {
    missing: Vec<Entry>,
    /// 상대가 더 최신인 파일과, 비교할 때의 내 파일 해시. 덮어쓰기 직전에 내 파일이 그대로인지 확인하는 기준이다.
    updated: Vec<(Entry, [u8; 32])>,
    deleted: Vec<Entry>,
    lost: Vec<LostConflict>,
}

impl SyncPlan {
    pub(super) fn new(
        mine: &[Entry],
        theirs: &[Entry],
        me: DeviceId,
        peer_id: Option<DeviceId>,
    ) -> Self {
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

        let lost = lost_conflicts(mine, &comparison.conflicts, me, peer_id);
        let updated = comparison
            .updated
            .into_iter()
            .filter_map(|entry| {
                let local = mine.iter().find(|local| local.path == entry.path)?;
                Some((entry, local.hash))
            })
            .collect();
        SyncPlan {
            transfers: Transfers {
                missing: comparison.missing,
                updated,
                deleted: comparison.deleted,
                lost,
            },
            merge: comparison.merge,
        }
    }
}

impl Transfers {
    /// 지우기, 충돌 해결, 받아 오기를 각각 작업으로 띄운다.
    /// 연결마다 따로 받으므로 한 연결에서 받는 중에도 다른 이벤트 처리가 막히지 않는다.
    pub(super) fn spawn(self, peer: PeerHandle, root: &Path, db_path: &Path, me: DeviceId) {
        if !self.deleted.is_empty() {
            tokio::spawn(delete_files(
                root.to_path_buf(),
                db_path.to_path_buf(),
                self.deleted,
            ));
        }
        if !self.lost.is_empty() {
            tokio::spawn(resolve_conflicts(
                peer.clone(),
                root.to_path_buf(),
                db_path.to_path_buf(),
                me,
                self.lost,
            ));
        }
        tokio::spawn(fetch_entries(
            peer,
            root.to_path_buf(),
            db_path.to_path_buf(),
            self.missing,
            self.updated,
        ));
    }
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

        let plan = SyncPlan::new(&mine, &theirs, DeviceId::from_cert(b"me"), None);

        assert_eq!(plan.transfers.missing, vec![entry("c.txt", 3)]);
        // 덮어쓰기 전에 확인할 수 있도록 비교할 때의 내 해시가 붙는다.
        assert_eq!(plan.transfers.updated, vec![(newer, [2; 32])]);
    }
}
