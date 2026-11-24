//! 내 폴더를 다루는 쪽: 루트 등록, 스냅샷, 파일 감시.
//! 상대 장치(`peer`)를 알지 못한다. 둘을 잇는 일은 앞으로 만들 coordinator가 맡는다.

mod root;
mod snapshot;
mod watcher;

pub use root::register_root;
pub use watcher::{watch_path, watch_root};
