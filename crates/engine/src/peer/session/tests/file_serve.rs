use super::*;

/// coordinator 역할을 대신하는 테스트용 파일 서버. `dir` 안의 파일을 요청받은 이름 그대로 열어 주고,
/// 없으면 `None`으로 답한다. 반환하는 송신구를 세션에 넘긴다.
fn spawn_file_server(dir: PathBuf) -> mpsc::Sender<PeerEvent> {
    let (events, mut requests) = mpsc::channel(8);
    tokio::spawn(async move {
        while let Some(event) = requests.recv().await {
            let PeerEvent::FileRequested { path, reply } = event else {
                continue;
            };
            let offer = match std::fs::read(dir.join(&path)) {
                Ok(bytes) => Some(FileOffer {
                    size: bytes.len() as u64,
                    hash: *blake3::hash(&bytes).as_bytes(),
                    file: tokio::fs::File::open(dir.join(&path)).await.unwrap(),
                }),
                Err(_) => None,
            };
            let _ = reply.send(offer);
        }
    });
    events
}

/// 상대 쪽에서 데몬과 핸드셰이크를 마치고, 파일 요청용 새 스트림을 열어 `path`를 요청한다.
/// 응답 헤더와, 헤더 뒤에 이어진 모든 바이트를 돌려준다. 연결은 호출한 쪽이 유지해야 한다.
async fn request_file(
    conn: &QuicConnection,
    path: &str,
) -> Result<(Message, Vec<u8>), Box<dyn Error>> {
    let (mut send, mut recv) = open_stream(conn).await?;
    write_handshake(&mut send).await?;
    read_handshake(&mut recv).await?;

    let (mut file_send, mut file_recv) = open_stream(conn).await?;
    write_frame(
        &mut file_send,
        &Message::FileRequest {
            path: path.to_owned(),
        },
    )
    .await?;

    let header = timeout(TIMEOUT, read_frame(&mut file_recv)).await??;
    // 데몬이 스트림을 끝낼 때까지 읽는다. 상한은 테스트 파일보다 넉넉하게 잡는다.
    let body = timeout(TIMEOUT, file_recv.read_to_end(16 * 1024 * 1024)).await??;
    Ok((header, body))
}

#[tokio::test]
async fn answers_a_file_request_with_header_and_raw_content() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let served = tempfile::tempdir()?;
    std::fs::create_dir(served.path().join("sub"))?;
    std::fs::write(served.path().join("sub/a.txt"), "hello")?;
    let events = spawn_file_server(served.path().to_owned());

    fixture
        .with_daemon_events(events, || async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            let (header, body) = request_file(&conn, "sub/a.txt").await?;

            assert_eq!(
                header,
                Message::FileFound {
                    size: 5,
                    hash: *blake3::hash(b"hello").as_bytes(),
                }
            );
            assert_eq!(body, b"hello");
            Ok(())
        })
        .await
}

#[tokio::test]
async fn sends_content_larger_than_the_frame_limit_as_raw_bytes() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let served = tempfile::tempdir()?;
    // 프레임 상한(1 MiB)보다 큰 3 MiB. 바이트마다 값이 달라야 순서가 틀리면 드러난다.
    let content: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(served.path().join("big.bin"), &content)?;
    let events = spawn_file_server(served.path().to_owned());

    fixture
        .with_daemon_events(events, || async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            let (header, body) = request_file(&conn, "big.bin").await?;

            assert_eq!(
                header,
                Message::FileFound {
                    size: content.len() as u64,
                    hash: *blake3::hash(&content).as_bytes(),
                }
            );
            assert!(body == content, "받은 내용이 원본과 다르다");
            Ok(())
        })
        .await
}

#[tokio::test]
async fn answers_not_found_when_the_coordinator_has_no_such_file() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let served = tempfile::tempdir()?;
    let events = spawn_file_server(served.path().to_owned());

    fixture
        .with_daemon_events(events, || async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            let (header, body) = request_file(&conn, "missing.txt").await?;

            assert_eq!(header, Message::FileNotFound);
            assert!(body.is_empty());
            Ok(())
        })
        .await
}
