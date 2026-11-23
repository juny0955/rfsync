use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;

mod codec;
mod error;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Message {
    Hello { version: u16 },
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
}
