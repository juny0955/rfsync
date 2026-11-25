use std::path::{Path, PathBuf};

use rfsync_protocol::Entry;
use tokio::task::spawn_blocking;
use tracing::{info, warn};

use crate::{
    error::EngineError,
    local::{OnExisting, clear_pending, record_pending, save_received_file},
    peer::PeerHandle,
    sync::compare::matches_entry,
};

/// 내게 없는 파일(`missing`)과 상대가 더 최신인 파일(`updated`)을 상대에게서 하나씩 받아 온다.
/// `updated`는 비교할 때의 내 파일(짝의 해시)이 그대로일 때만 덮어쓴다. 한 파일이 실패해도 나머지는 계속 받는다.
///
/// 파일마다 새 스트림을 쓰므로 동시에 받을 수도 있지만, 지금은 순서대로 하나씩 받는다.
pub(super) async fn fetch_entries(
    peer: PeerHandle,
    root: PathBuf,
    db_path: PathBuf,
    missing: Vec<Entry>,
    updated: Vec<(Entry, [u8; 32])>,
) {
    let targets = missing.iter().map(|entry| (entry, OnExisting::Fail)).chain(
        updated
            .iter()
            .map(|(entry, local_hash)| (entry, OnExisting::ReplaceIfHash(*local_hash))),
    );
    for (entry, on_existing) in targets {
        match fetch_file(&peer, &root, &db_path, entry, on_existing).await {
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
            Ok(FetchOutcome::ChangedLocally) => info!(
                "받는 동안 내 파일이 바뀌어 덮어쓰지 않았습니다: {}",
                entry.path
            ),
            Err(e) => warn!("파일을 받지 못했습니다: {}: {e}", entry.path),
        }
    }
}

/// 파일 하나를 받으려던 결과.
#[derive(Debug, PartialEq)]
pub(super) enum FetchOutcome {
    Saved,
    /// 목록에는 있었는데 요청하니 상대가 없다고 답했다.
    MissingOnPeer,
    /// 상대가 알려 준 크기나 해시가 목록의 값과 달라, 목록을 받은 뒤 파일이 바뀐 것이다.
    ChangedOnPeer,
    /// 받는 동안 내 파일이 바뀌어 덮어쓰지 않았다. 다음 비교가 새 상태로 다시 판단한다.
    ChangedLocally,
}

/// 파일 하나를 요청하고, 응답 헤더가 목록의 값과 같을 때만 내용을 받아 저장한다.
pub(super) async fn fetch_file(
    peer: &PeerHandle,
    root: &Path,
    db_path: &Path,
    entry: &Entry,
    on_existing: OnExisting,
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
        on_existing,
        &mut download.reader,
    )
    .await;
    if let Err(e) = saved {
        // 파일이 생기지 않았으므로 기록도 지운다.
        clear_pending_logged(db_path, &entry.path).await;
        return match e {
            EngineError::LocalChanged(_) => Ok(FetchOutcome::ChangedLocally),
            e => Err(e),
        };
    }
    Ok(FetchOutcome::Saved)
}

/// 남은 `sync_pending` 기록을 지운다. 지우지 못해도 받는 일은 계속하므로 로그만 남긴다.
pub(super) async fn clear_pending_logged(db_path: &Path, path: &str) {
    let (db_path, path) = (db_path.to_path_buf(), path.to_owned());
    let logged = path.clone();
    match spawn_blocking(move || clear_pending(&db_path, &path)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("남은 수신 기록을 지우지 못했습니다: {logged}: {e}"),
        Err(e) => warn!("수신 기록 지우기 작업이 실패했습니다: {logged}: {e}"),
    }
}
