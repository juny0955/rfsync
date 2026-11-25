//! 연결 직후 한 번 하는 핸드셰이크: `Hello`와 파일 목록을 한 스트림으로 주고받는다.

use rfsync_network::{QuicConnection, QuicRecvStream, QuicSendStream, accept_stream, open_stream};
use rfsync_protocol::{Entry, Message, PROTOCOL_VERSION, read_frame, write_frame};
use tokio::io::{AsyncRead, AsyncWrite};

use super::SessionRole;
use crate::error::EngineError;

/// 연결에서 쓰는 대화 창구. 핸드셰이크 뒤에도 같은 스트림으로 계속 주고받는다.
pub(super) struct ControlStream {
    pub(super) send: QuicSendStream,
    pub(super) recv: QuicRecvStream,
}

/// 상대가 보낸 핸드셰이크 내용.
pub(super) struct PeerHandshake {
    pub(super) version: u16,
    pub(super) index: Vec<Entry>,
}

/// 연결을 시작한 쪽은 스트림을 열고, 받은 쪽은 스트림을 받아 `Hello`와 파일 목록을 주고받는다.
pub(super) async fn handshake(
    conn: &QuicConnection,
    role: SessionRole,
    index: &[Entry],
) -> Result<(ControlStream, PeerHandshake), EngineError> {
    let (mut send, mut recv) = match role {
        SessionRole::Dialer => open_stream(conn).await?,
        SessionRole::Acceptor => accept_stream(conn).await?,
    };

    let peer = exchange(&mut send, &mut recv, index).await?;
    Ok((ControlStream { send, recv }, peer))
}

/// `Hello`와 파일 목록을 보내는 일과 상대의 것을 받는 일을 동시에 한다.
/// 둘 다 쓰고 난 뒤에 읽으면 큰 목록에서 서로 상대가 읽기를 기다리다 멈출 수 있기 때문이다.
pub(super) async fn exchange(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    index: &[Entry],
) -> Result<PeerHandshake, EngineError> {
    let write = async {
        write_frame(
            send,
            &Message::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .await?;
        write_frame(
            send,
            &Message::Index {
                entries: index.to_vec(),
            },
        )
        .await?;
        Ok::<(), EngineError>(())
    };

    let read = async {
        let Message::Hello { version } = read_frame(recv).await? else {
            return Err(EngineError::UnexpectedMessage("Hello"));
        };
        let Message::Index { entries } = read_frame(recv).await? else {
            return Err(EngineError::UnexpectedMessage("Index"));
        };
        Ok::<PeerHandshake, EngineError>(PeerHandshake {
            version,
            index: entries,
        })
    };

    let ((), peer) = tokio::try_join!(write, read)?;
    Ok(peer)
}
