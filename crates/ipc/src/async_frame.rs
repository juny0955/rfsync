use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    IpcError,
    frame::{check_len, encode},
};

/// `frame::write_frame`과 같은 형식으로 쓴다.
pub async fn write_frame_async(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> Result<(), IpcError> {
    let bytes = encode(value)?;
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// `frame::read_frame`과 같은 형식으로 읽는다.
pub async fn read_frame_async<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = check_len(u32::from_be_bytes(len_buf) as usize)?;

    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    Ok(postcard::from_bytes(&bytes)?)
}
