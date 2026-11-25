use std::{net::SocketAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

/// CLI가 데몬에 보내는 요청. 연결 하나에 요청 하나를 보내고 응답을 읽는다.
///
/// 변형은 순서대로 직렬화되므로 새 변형은 항상 맨 뒤에 추가한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// 데몬이 살아 있는지 확인한다. 응답은 `Ok`다.
    Ping,
    /// 폴더를 동기화 루트로 등록한다. 응답은 `Ok`다.
    ///
    /// 상대 경로의 기준이 CLI와 데몬에서 다를 수 있으므로 CLI가 절대 경로로 바꿔 보낸다.
    AddRoot { path: PathBuf },
    /// 페어링을 기다린다. 데몬은 만든 코드를 `PairingCode`로 먼저 보내고, 끝나면 `Ok`나 `Error`를 보낸다.
    PairListen,
    /// 상대의 주소와 코드로 페어링한다. 응답은 `Ok`다.
    PairConnect { addr: SocketAddr, code: String },
}

/// 데몬이 CLI에 보내는 응답. 요청 하나에 하나 이상을 보낼 수 있다.
///
/// 변형은 순서대로 직렬화되므로 새 변형은 항상 맨 뒤에 추가한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// 요청을 마쳤다. 요청 하나의 마지막 응답이다.
    Ok,
    /// 페어링 코드. 사용자에게 보여 줄 값이며, 같은 요청의 마지막 응답이 뒤따른다.
    PairingCode(String),
    /// 요청이 실패했다. 사람이 읽을 메시지이며, 요청 하나의 마지막 응답이다.
    Error(String),
}
