use std::io::Cursor;

use super::*;
use crate::frame::MAX_FRAME_SIZE;

fn requests() -> Vec<Request> {
    vec![
        Request::Ping,
        Request::AddRoot {
            path: "/home/user/docs".into(),
        },
        Request::PairListen,
        Request::PairConnect {
            addr: "192.168.0.7:10000".parse().unwrap(),
            code: "123456".into(),
        },
    ]
}

fn responses() -> Vec<Response> {
    vec![
        Response::Ok,
        Response::PairingCode("123456".into()),
        Response::Error("이미 등록된 root입니다".into()),
    ]
}

#[test]
fn 쓰고_읽으면_같은_요청() {
    for request in requests() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();

        let read: Request = read_frame(&mut Cursor::new(bytes)).unwrap();

        assert_eq!(read, request);
    }
}

#[test]
fn 쓰고_읽으면_같은_응답() {
    for response in responses() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();

        let read: Response = read_frame(&mut Cursor::new(bytes)).unwrap();

        assert_eq!(read, response);
    }
}

#[test]
fn 연달아_쓴_프레임은_각각_분리해서_읽힌다() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &Response::PairingCode("123456".into())).unwrap();
    write_frame(&mut bytes, &Response::Ok).unwrap();
    let mut reader = Cursor::new(bytes);

    assert_eq!(
        read_frame::<Response>(&mut reader).unwrap(),
        Response::PairingCode("123456".into())
    );
    assert_eq!(read_frame::<Response>(&mut reader).unwrap(), Response::Ok);
}

#[test]
fn 프레임_앞에는_4바이트_빅_엔디언_길이가_붙는다() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &Request::Ping).unwrap();

    assert_eq!(bytes, [0, 0, 0, 1, 0]);
}

#[test]
fn 기존_변형의_바이트_표현은_변형을_추가해도_그대로다() {
    // 변형은 순서대로 직렬화된다. 이 값이 바뀌면 이미 떠 있는 데몬과 CLI가 서로 못 읽는다.
    let encoded = |request: &Request| postcard::to_stdvec(request).unwrap()[0];
    assert_eq!(encoded(&Request::Ping), 0);
    assert_eq!(encoded(&requests()[1]), 1);
    assert_eq!(encoded(&Request::PairListen), 2);
    assert_eq!(encoded(&requests()[3]), 3);

    let encoded = |response: &Response| postcard::to_stdvec(response).unwrap()[0];
    assert_eq!(encoded(&Response::Ok), 0);
    assert_eq!(encoded(&Response::PairingCode(String::new())), 1);
    assert_eq!(encoded(&Response::Error(String::new())), 2);
}

#[test]
fn 상한보다_큰_길이를_알리는_프레임은_읽지_않고_거부한다() {
    let len = (MAX_FRAME_SIZE as u32 + 1).to_be_bytes();

    let result = read_frame::<Request>(&mut Cursor::new(len));

    assert!(matches!(result, Err(IpcError::FrameTooLarge { .. })));
}

#[test]
fn 상한보다_큰_값은_쓰지_않고_거부한다() {
    let huge = Response::Error("가".repeat(MAX_FRAME_SIZE));
    let mut bytes = Vec::new();

    let result = write_frame(&mut bytes, &huge);

    assert!(matches!(result, Err(IpcError::FrameTooLarge { .. })));
    assert!(bytes.is_empty());
}

#[test]
fn 본문이_길이만큼_오기_전에_끊기면_에러다() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &requests()[1]).unwrap();
    bytes.truncate(bytes.len() - 1);

    let result = read_frame::<Request>(&mut Cursor::new(bytes));

    assert!(matches!(result, Err(IpcError::Io(_))));
}

#[test]
fn 읽을_수_없는_본문은_에러다() {
    // 길이는 1이고 본문 0xFF는 어떤 요청 변형도 아니다.
    let bytes = [0, 0, 0, 1, 0xFF];

    let result = read_frame::<Request>(&mut Cursor::new(bytes));

    assert!(matches!(result, Err(IpcError::Postcard(_))));
}

#[tokio::test]
async fn 비동기로_쓰고_읽으면_같은_요청() {
    let (mut a, mut b) = tokio::io::duplex(1024);

    for request in requests() {
        write_frame_async(&mut a, &request).await.unwrap();

        let read: Request = read_frame_async(&mut b).await.unwrap();

        assert_eq!(read, request);
    }
}

#[tokio::test]
async fn 동기로_쓴_프레임을_비동기로_읽을_수_있다() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &Response::PairingCode("123456".into())).unwrap();
    let (mut a, mut b) = tokio::io::duplex(1024);
    tokio::io::AsyncWriteExt::write_all(&mut a, &bytes)
        .await
        .unwrap();

    let read: Response = read_frame_async(&mut b).await.unwrap();

    assert_eq!(read, Response::PairingCode("123456".into()));
}

#[tokio::test]
async fn 비동기로_쓴_프레임을_동기로_읽을_수_있다() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    write_frame_async(&mut a, &Response::Ok).await.unwrap();
    drop(a);
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut b, &mut bytes)
        .await
        .unwrap();

    let read: Response = read_frame(&mut Cursor::new(bytes)).unwrap();

    assert_eq!(read, Response::Ok);
}

#[cfg(unix)]
#[test]
fn 클라이언트는_요청을_보내고_응답_하나를_읽는다() {
    let dir = std::env::temp_dir().join(format!("rfsync-ipc-client-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("test.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request: Request = read_frame(&mut stream).unwrap();
        write_frame(&mut stream, &Response::Error(format!("{request:?}"))).unwrap();
    });

    let response = crate::request(&socket, &Request::Ping).unwrap();

    server.join().unwrap();
    assert_eq!(response, Response::Error("Ping".into()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn 소켓이_없으면_클라이언트는_io_에러를_돌려준다() {
    let missing = std::env::temp_dir().join("rfsync-ipc-no-such.sock");

    assert!(matches!(
        crate::request(&missing, &Request::Ping),
        Err(crate::IpcError::Io(_))
    ));
}
