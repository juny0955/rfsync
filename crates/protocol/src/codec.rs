use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Message, ProtocolError, decode, encode};

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

    #[tokio::test]
    async fn 연달아_쓴_메시지는_각각_분리해서_읽힌다() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let first = Message::Hello { version: 1 };
        let second = Message::Hello { version: 2 };

        write_frame(&mut a, &first).await.unwrap();
        write_frame(&mut a, &second).await.unwrap();

        assert_eq!(read_frame(&mut b).await.unwrap(), first);
        assert_eq!(read_frame(&mut b).await.unwrap(), second);
    }

    #[tokio::test]
    async fn 상한을_넘는_길이는_거절한다() {
        let (mut a, mut b) = tokio::io::duplex(1024);

        // 내용 없이 "길이: u32::MAX"라는 4바이트만 보낸다
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();

        let result = read_frame(&mut b).await;
        assert!(matches!(result, Err(ProtocolError::FrameTooLarge { .. })));
    }
}
