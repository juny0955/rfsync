use std::{os::unix::net::UnixStream, path::Path};

use crate::{IpcError, Request, Response, read_frame, write_frame};

/// 데몬 소켓에 접속해 요청 하나를 보내고 마지막 응답(`Ok`나 `Error`)을 읽는다.
/// 마지막 응답 앞에 오는 응답은 버린다. 중간 응답이 필요하면 `request_streaming`을 쓴다.
pub fn request(socket: &Path, request: &Request) -> Result<Response, IpcError> {
    request_streaming(socket, request, |_| {})
}

/// `request`와 같지만, 마지막 응답 앞에 오는 응답(예: `PairingCode`)을 오는 대로 `on_progress`에 넘긴다.
pub fn request_streaming(
    socket: &Path,
    request: &Request,
    mut on_progress: impl FnMut(&Response),
) -> Result<Response, IpcError> {
    let mut stream = UnixStream::connect(socket)?;
    write_frame(&mut stream, request)?;
    loop {
        let response: Response = read_frame(&mut stream)?;
        match response {
            Response::Ok | Response::Error(_) => return Ok(response),
            _ => on_progress(&response),
        }
    }
}
