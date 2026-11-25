use std::{os::unix::net::UnixStream, path::Path};

use crate::{IpcError, Request, Response, read_frame, write_frame};

/// 데몬 소켓에 접속해 요청 하나를 보내고 응답 하나를 읽는다. 응답이 하나인 요청에 쓴다.
pub fn request(socket: &Path, request: &Request) -> Result<Response, IpcError> {
    let mut stream = UnixStream::connect(socket)?;
    write_frame(&mut stream, request)?;
    read_frame(&mut stream)
}
