use serde::{Deserialize, Serialize};

use crate::ProtocolError;

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    /// 루트 기준 상대 경로. 구분자는 항상 `/`다.
    pub path: String,
    pub size: u64,
    pub hash: [u8; 32],
    pub modified_secs: i64,
    pub modified_nanos: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Message {
    Hello {
        version: u16,
    },
    Index {
        entries: Vec<Entry>,
    },
    /// 파일 스트림에서 요청하는 쪽이 처음 보낸다. 파일 하나를 달라는 요청이다.
    /// `path`는 `Entry.path`와 같은 규칙(루트 기준 상대 경로, `/` 구분자)이며, 받는 쪽이 다시 검증한다.
    FileRequest {
        path: String,
    },
    /// 요청한 파일이 있다. 이 프레임 바로 뒤에 `size` 바이트의 파일 내용이 프레임 없이 이어진다.
    FileFound {
        size: u64,
        hash: [u8; 32],
    },
    /// 요청한 파일이 없다. 뒤에 이어지는 내용은 없다.
    FileNotFound,
    /// 연결된 뒤에 바뀐 항목만 보낸다. `Index`와 달리 전체 목록이 아니다.
    /// 변형은 순서 번호로 직렬화되므로 새 변형은 항상 맨 뒤에 추가한다.
    IndexUpdate {
        entries: Vec<Entry>,
    },
}

pub fn encode(msg: &Message) -> Result<Vec<u8>, ProtocolError> {
    Ok(postcard::to_stdvec(msg)?)
}

pub fn decode(bytes: &[u8]) -> Result<Message, ProtocolError> {
    Ok(postcard::from_bytes(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 메시지를_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::Hello { version: 1 };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn index_메시지를_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::Index {
            entries: vec![
                Entry {
                    path: "docs/a.txt".to_string(),
                    size: 10,
                    hash: [7u8; 32], // 7이 32개인 가짜 해시
                    modified_secs: 1_700_000_000,
                    modified_nanos: 500,
                },
                Entry {
                    path: "docs/b.txt".to_string(),
                    size: 10,
                    hash: [7u8; 32], // 7이 32개인 가짜 해시
                    modified_secs: 1_700_000_000,
                    modified_nanos: 500,
                },
            ],
        };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn 파일_요청을_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::FileRequest {
            path: "sub/한글.txt".to_string(),
        };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn 파일_응답을_바이트로_바꿨다_되돌리면_같다() {
        let found = Message::FileFound {
            size: u64::MAX,
            hash: [7u8; 32],
        };
        let not_found = Message::FileNotFound;
        assert_eq!(decode(&encode(&found).unwrap()).unwrap(), found);
        assert_eq!(decode(&encode(&not_found).unwrap()).unwrap(), not_found);
    }

    #[test]
    fn index_update를_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::IndexUpdate {
            entries: vec![Entry {
                path: "sub/새 파일.txt".to_string(),
                size: 5,
                hash: [3u8; 32],
                modified_secs: 1_700_000_000,
                modified_nanos: 1,
            }],
        };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn 기존_메시지의_바이트_표현은_변형을_추가해도_그대로다() {
        // 변형은 순서 번호로 직렬화되므로 새 변형은 뒤에만 추가해야 한다. 앞 번호가 밀리지 않는지 확인한다.
        assert_eq!(encode(&Message::Hello { version: 1 }).unwrap(), [0, 1]);
        assert_eq!(encode(&Message::Index { entries: vec![] }).unwrap(), [1, 0]);
    }

    #[test]
    fn 빈_index도_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::Index { entries: vec![] };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }
}
