use std::io::{Read, Write};

use serde::{Serialize, de::DeserializeOwned};

use crate::IpcError;

/// 프레임 하나의 최대 크기. 요청과 응답은 작아서 넉넉하게 잡았다.
pub(crate) const MAX_FRAME_SIZE: usize = 64 * 1024;

/// 값을 4바이트 길이(빅 엔디언)와 postcard 본문으로 쓴다.
pub fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<(), IpcError> {
    let bytes = encode(value)?;
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// `write_frame`이 쓴 프레임 하나를 읽는다.
pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = check_len(u32::from_be_bytes(len_buf) as usize)?;

    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    Ok(postcard::from_bytes(&bytes)?)
}

pub(crate) fn encode(value: &impl Serialize) -> Result<Vec<u8>, IpcError> {
    let bytes = postcard::to_stdvec(value)?;
    check_len(bytes.len())?;
    Ok(bytes)
}

pub(crate) fn check_len(len: usize) -> Result<usize, IpcError> {
    if len > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_SIZE,
        });
    }
    Ok(len)
}
