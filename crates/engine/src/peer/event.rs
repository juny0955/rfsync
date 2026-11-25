use rfsync_protocol::Entry;
use tokio::{fs::File, sync::oneshot};

use super::handle::PeerHandle;

/// 세션이 coordinator에게 알리는 일.
#[derive(Debug)]
pub enum PeerEvent {
    /// 핸드셰이크에서 상대의 파일 목록을 받았다. `peer`로 이 연결에 파일을 요청할 수 있다.
    IndexReceived { index: Vec<Entry>, peer: PeerHandle },
    /// 연결된 뒤에 상대의 파일이 바뀌었다. 바뀐 항목만 들어 있고, `peer`로 이 연결에 파일을 요청할 수 있다.
    IndexUpdated {
        entries: Vec<Entry>,
        peer: PeerHandle,
    },
    /// 상대가 파일 하나를 요청했다. 답은 `reply`로 돌려받는다. 줄 수 없는 파일이면 `None`이다.
    FileRequested {
        path: String,
        reply: oneshot::Sender<Option<FileOffer>>,
    },
}

/// 상대에게 보낼 수 있게 열어 둔 파일. 읽는 위치는 맨 앞이다.
#[derive(Debug)]
pub struct FileOffer {
    pub size: u64,
    pub hash: [u8; 32],
    pub file: File,
}
