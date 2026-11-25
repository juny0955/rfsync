//! CLI의 요청을 받는 로컬 소켓. 같은 기기에서 같은 사용자만 접속할 수 있다(권한 0600).

use std::path::PathBuf;

use tokio::sync::mpsc::Sender;

mod handlers;
mod server;
#[cfg(test)]
mod tests;

pub(crate) use server::start;

/// 요청을 처리하는 데 필요한 데몬의 상태와, 처리한 결과를 데몬 본체에 알리는 송신구.
pub(crate) struct IpcContext {
    pub db_path: PathBuf,
    /// 페어링 접속을 기다리는 TCP 포트. 데몬의 QUIC 포트와 같다.
    pub port: u16,
    pub cert: Vec<u8>,
    /// 루트를 등록했다는 알림. 쌓여 있으면 받는 쪽이 곧 최신 상태를 읽으므로 가득 차도 괜찮다.
    pub root_added: Sender<()>,
    /// 장치를 페어링했다는 알림.
    pub devices_changed: Sender<()>,
}
