//! 상대 장치와 통신하는 쪽: 장치 신원, 페어링, 세션, 파일 목록(index).
//! 내 폴더(`local`)를 알지 못한다. 둘을 잇는 일은 앞으로 만들 coordinator가 맡는다.

mod event;
mod handle;
mod identity;
mod index;
mod pairing;
mod session;

pub use event::{FileOffer, PeerEvent};
pub use handle::PeerHandle;
pub use identity::{Identity, load_or_create_identity};
pub(crate) use index::load_index;
pub use pairing::{connect, listen};
pub(crate) use pairing::{pair_connect, pair_listen};
pub(crate) use session::run;
