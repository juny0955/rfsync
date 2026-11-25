use std::path::{Path, PathBuf};

use rfsync_protocol::{DeviceId, Entry};
use tracing::{debug, info, warn};

use crate::{
    local::{OnExisting, PreserveOutcome, preserve_file},
    peer::PeerHandle,
    sync::{
        blocking,
        conflict::{Winner, conflict_path, decide_winner},
        fetch::{FetchOutcome, fetch_file},
    },
};

/// 충돌에서 내가 진 파일. 내 파일과 이긴 상대 파일의 짝이다.
pub(super) struct LostConflict {
    mine: Entry,
    theirs: Entry,
}

/// 동시에 고친 파일 중 내가 진 것만 골라낸다. 이긴 쪽은 아무것도 하지 않는다. 진 쪽이 내용을 남기고
/// 이긴 쪽 파일을 받아 가면 양쪽이 같아진다. 상대 장치를 알 수 없으면 승자를 가릴 수 없어 건드리지 않는다.
pub(super) fn lost_conflicts(
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
pub(super) async fn resolve_conflicts(
    peer: PeerHandle,
    root: PathBuf,
    db_path: PathBuf,
    me: DeviceId,
    lost: Vec<LostConflict>,
) {
    for LostConflict { mine, theirs } in lost {
        let Some((preserved, outcome)) = preserve_loser(&root, &mine, me).await else {
            continue;
        };
        match outcome {
            PreserveOutcome::Preserved(_) => {
                info!("동시에 고쳐 내가 져서 내 파일을 남겼습니다: {preserved}")
            }
            PreserveOutcome::SourceMissing => debug!("남길 파일이 이미 없습니다: {}", mine.path),
            // `AlreadyExists`는 `preserve_loser`가 이름을 바꿔 다시 시도하므로 여기까지 오지 않는다.
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
        // 남긴 내용 그대로일 때만 덮어쓴다. 남긴 뒤 받는 동안 또 고쳤다면 그 수정을 잃지 않도록 멈춘다.
        let on_existing = match outcome {
            PreserveOutcome::Preserved(hash) => OnExisting::ReplaceIfHash(hash),
            _ => OnExisting::Fail,
        };
        match fetch_file(&peer, &root, &db_path, &incoming, on_existing).await {
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
            Ok(FetchOutcome::ChangedLocally) => info!(
                "남긴 뒤 내 파일이 또 바뀌어 덮어쓰지 않았습니다: {}",
                incoming.path
            ),
            Err(e) => warn!(
                "충돌에서 이긴 파일을 받지 못했습니다: {}: {e}",
                incoming.path
            ),
        }
    }
}

/// 같은 이름에 다른 내용이 이미 있을 때 이름을 바꿔 가며 시도하는 최대 횟수.
const MAX_PRESERVE_ATTEMPTS: u32 = 10;

/// 내가 진 파일을 남긴다. 이름에 다른 내용이 이미 있으면 번호를 붙여 다른 이름으로 다시 시도한다.
/// 남긴 이름과 결과를 돌려주며, 실패하면 이유를 로그에 남기고 `None`이다.
async fn preserve_loser(
    root: &Path,
    mine: &Entry,
    me: DeviceId,
) -> Option<(String, PreserveOutcome)> {
    for attempt in 0..MAX_PRESERVE_ATTEMPTS {
        let preserved = conflict_path(&mine.path, mine.modified_secs, me, attempt);
        let (root, path, name) = (root.to_path_buf(), mine.path.clone(), preserved.clone());
        match blocking(move || preserve_file(&root, &path, &name)).await {
            Ok(PreserveOutcome::AlreadyExists) => continue,
            Ok(outcome) => return Some((preserved, outcome)),
            Err(e) => {
                warn!(
                    "진 파일을 남기지 못해 덮어쓰지 않습니다: {}: {e}",
                    mine.path
                );
                return None;
            }
        }
    }
    warn!(
        "진 파일을 남길 이름을 찾지 못해 덮어쓰지 않습니다: {}",
        mine.path
    );
    None
}
