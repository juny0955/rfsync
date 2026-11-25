use rfsync_network::{QuicConnection, QuicRecvStream, open_stream};
use rfsync_protocol::{Entry, Message, read_frame, write_frame};
use tokio::sync::mpsc::Sender;

use crate::error::EngineError;

/// 상대가 주기로 한 파일. `reader`에서 정확히 `size`바이트를 읽으면 파일 내용이 끝난다.
#[derive(Debug)]
pub struct FileDownload {
    /// 상대가 알려 준 크기. 목록의 값과 같은지는 받는 쪽이 확인한다.
    pub size: u64,
    /// 상대가 알려 준 내용의 해시. 받은 내용으로 다시 계산해서 비교해야 한다.
    pub hash: [u8; 32],
    pub reader: QuicRecvStream,
}

/// 상대와 맺은 연결을 가리키는 손잡이. 복제해서 다른 작업에 넘길 수 있다.
///
/// 파일을 받으려면 coordinator가 이 손잡이로 요청을 보낸다. 세션은 요청을 보내고 응답 헤더를 읽는
/// 데까지만 하고, 받은 내용을 어디에 저장할지는 정하지 않는다(내 폴더를 알지 못한다).
#[derive(Debug, Clone)]
pub struct PeerHandle {
    conn: QuicConnection,
    /// 세션의 제어 스트림 쓰기 작업으로 가는 통로. 보낼 항목을 여기에 넣으면 세션이 `IndexUpdate`로 보낸다.
    updates: Sender<Vec<Entry>>,
}

impl PeerHandle {
    pub(super) fn new(conn: QuicConnection, updates: Sender<Vec<Entry>>) -> Self {
        Self { conn, updates }
    }

    /// 연결된 뒤에 바뀐 항목을 상대에게 알린다. 연결이 이미 끝났다면 에러다.
    pub async fn send_update(&self, entries: Vec<Entry>) -> Result<(), EngineError> {
        self.updates
            .send(entries)
            .await
            .map_err(|_| EngineError::PeerDisconnected)
    }

    /// 상대에게 파일 하나를 요청한다. `path`는 루트 기준 상대 경로(`Entry.path`)다.
    ///
    /// 요청마다 새 스트림을 열어, 다른 파일의 전송과 서로 기다리지 않는다.
    /// 상대에게 그 파일이 없으면 `Ok(None)`이다. 응답 헤더 뒤의 내용은 읽지 않고 `FileDownload`로 넘긴다.
    pub async fn request_file(&self, path: &str) -> Result<Option<FileDownload>, EngineError> {
        let (mut send, mut recv) = open_stream(&self.conn).await?;
        write_frame(
            &mut send,
            &Message::FileRequest {
                path: path.to_owned(),
            },
        )
        .await?;
        // 보낼 것은 이 요청 하나뿐이다. 이미 닫혔다면 아래 읽기에서 에러로 드러난다.
        let _ = send.finish();

        match read_frame(&mut recv).await? {
            Message::FileFound { size, hash } => Ok(Some(FileDownload {
                size,
                hash,
                reader: recv,
            })),
            Message::FileNotFound => Ok(None),
            _ => Err(EngineError::UnexpectedMessage(
                "FileFound 또는 FileNotFound",
            )),
        }
    }
}
