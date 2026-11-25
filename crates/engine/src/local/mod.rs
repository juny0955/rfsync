//! 내 폴더를 다루는 쪽: 루트 등록, 스냅샷, 파일 감시.
//! 상대 장치(`peer`)를 알지 못한다. 둘을 잇는 일은 앞으로 만들 coordinator가 맡는다.

mod merge;
mod path;
mod pending;
mod receive;
mod remove;
mod root;
mod serve;
mod snapshot;
mod watcher;

pub(crate) use merge::merge_versions;
pub(crate) use pending::{clear_pending, record_pending};
pub(crate) use receive::save_received_file;
pub(crate) use remove::{DeleteOutcome, delete_if_unchanged};
pub(crate) use root::first_root_path;
pub use root::register_root;
pub(crate) use serve::open_for_serving;
pub(crate) use watcher::spawn_watcher;
pub use watcher::watch_root;
