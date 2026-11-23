use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Message, decode, encode, error::ProtocolError};

const MAX_FRAME_SIZE: usize = 1024 * 1024; // 1 MiB

pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    msg: &Message,
) -> Result<(), ProtocolError> {
    let bytes = encode(msg)?;
    if bytes.len() > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: bytes.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    let len = bytes.len() as u32;

    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    Ok(())
}

pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Message, ProtocolError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_SIZE,
        });
    }

    let mut bytes = vec![0u8; len];

    reader.read_exact(&mut bytes).await?;
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 쓰고_읽으면_같은_메시지() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let msg = Message::Hello { version: 1 };

        write_frame(&mut a, &msg).await.unwrap();

        assert_eq!(read_frame(&mut b).await.unwrap(), msg);
    }
}
