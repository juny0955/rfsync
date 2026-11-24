use rfsync_protocol::Entry;

/// 세션이 coordinator에게 알리는 일.
#[derive(Debug)]
pub enum PeerEvent {
    /// 핸드셰이크에서 상대의 파일 목록을 받았다.
    IndexReceived { index: Vec<Entry> },
}
