use std::path::PathBuf;

use tokio::fs::File;
use tracing::{debug, warn};

use crate::{local::open_for_serving, peer::FileOffer, sync::blocking};

/// 요청받은 파일을 열어 크기와 해시를 계산한다. 줄 수 없으면 이유를 로그에 남기고 `None`이다.
/// 상대에게는 이유를 알리지 않는다(없는 파일과 거절한 파일을 구분해 주지 않는다).
pub(super) async fn serve_file(root: Option<PathBuf>, requested: String) -> Option<FileOffer> {
    let root = root?;
    let path = requested.clone();

    // 파일을 읽고 해시를 계산하는 일은 오래 걸릴 수 있어 블로킹 전용 스레드에서 한다.
    let served = match blocking(move || open_for_serving(&root, &requested)).await {
        Ok(Some(served)) => served,
        Ok(None) => {
            debug!("요청받은 파일이 없습니다: {path:?}");
            return None;
        }
        Err(e) => {
            warn!("요청받은 파일을 줄 수 없습니다: {path:?}: {e}");
            return None;
        }
    };

    Some(FileOffer {
        size: served.size,
        hash: served.hash,
        file: File::from_std(served.file),
    })
}
