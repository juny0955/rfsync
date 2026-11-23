use serde::{Deserialize, Serialize};

use crate::ProtocolError;

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub hash: [u8; 32],
    pub modified_secs: i64,
    pub modified_nanos: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Message {
    Hello { version: u16 },
    Index { entries: Vec<Entry> },
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
    fn 빈_index도_바이트로_바꿨다_되돌리면_같다() {
        let msg = Message::Index { entries: vec![] };
        let bytes = encode(&msg).unwrap();
        assert_eq!(decode(&bytes).unwrap(), msg);
    }
}
