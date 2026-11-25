use std::path::PathBuf;

use rfsync_protocol::Entry;
use tokio::task::spawn_blocking;
use tracing::{debug, info, warn};

use crate::{
    local::{DeleteOutcome, delete_if_unchanged, record_pending},
    sync::fetch::clear_pending_logged,
};

/// 상대가 지운 파일을 내 폴더에서 지운다. 내용이 목록의 해시와 같은 파일만 지우며, 한 파일이 실패해도 나머지는 계속한다.
///
/// 지운 뒤의 삭제 기록은 감시가 폴더 변화를 보고 DB에 남기고, 상대에게도 같은 경로로 알린다.
pub(super) async fn delete_files(root: PathBuf, db_path: PathBuf, deleted: Vec<Entry>) {
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
