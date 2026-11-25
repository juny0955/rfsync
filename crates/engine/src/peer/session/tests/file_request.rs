use super::*;

#[tokio::test]
async fn 핸들을_통해_피어에게_파일을_요청한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let peer = handshake_with_daemon(&fixture, &mut received).await?;

            let serve = answer_request(&peer.conn, "sub/a.txt", |mut send| async move {
                write_frame(
                    &mut send,
                    &Message::FileFound {
                        size: 5,
                        hash: *blake3::hash(b"hello").as_bytes(),
                    },
                )
                .await?;
                send.write_all(b"hello").await?;
                send.finish()?;
                Ok(())
            });
            let request = async {
                let mut download = timeout(TIMEOUT, peer.handle.request_file("sub/a.txt"))
                    .await??
                    .ok_or("파일이 없다고 답했다")?;
                assert_eq!(download.size, 5);
                assert_eq!(download.hash, *blake3::hash(b"hello").as_bytes());
                let content = timeout(TIMEOUT, download.reader.read_to_end(1024)).await??;
                assert_eq!(content, b"hello");
                Ok::<(), Box<dyn Error>>(())
            };

            let (served, requested) = tokio::join!(serve, request);
            served?;
            requested
        })
        .await
}

#[tokio::test]
async fn 피어에게_파일이_없으면_none을_반환한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let peer = handshake_with_daemon(&fixture, &mut received).await?;

            let serve = answer_request(&peer.conn, "missing.txt", |mut send| async move {
                write_frame(&mut send, &Message::FileNotFound).await?;
                send.finish()?;
                Ok(())
            });
            let request = async {
                let download = timeout(TIMEOUT, peer.handle.request_file("missing.txt")).await??;
                assert!(download.is_none());
                Ok::<(), Box<dyn Error>>(())
            };

            let (served, requested) = tokio::join!(serve, request);
            served?;
            requested
        })
        .await
}

#[tokio::test]
async fn 피어가_예상치_못한_메시지로_답하면_실패한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let peer = handshake_with_daemon(&fixture, &mut received).await?;

            let serve = answer_request(&peer.conn, "a.txt", |mut send| async move {
                write_frame(&mut send, &hello()).await?;
                send.finish()?;
                Ok(())
            });
            let request = async {
                let result = timeout(TIMEOUT, peer.handle.request_file("a.txt")).await?;
                assert!(matches!(result, Err(EngineError::UnexpectedMessage(_))));
                Ok::<(), Box<dyn Error>>(())
            };

            let (served, requested) = tokio::join!(serve, request);
            served?;
            requested
        })
        .await
}

#[tokio::test]
async fn 요청마다_별도_스트림을_써서_다운로드가_겹칠_수_있다() -> Result<(), Box<dyn Error>>
{
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let peer = handshake_with_daemon(&fixture, &mut received).await?;

            // 상대는 두 요청을 모두 받은 뒤에야 답한다. 요청마다 스트림이 따로여야 둘 다 도착한다.
            let serve = async {
                let mut pending = Vec::new();
                for _ in 0..2 {
                    let (send, mut recv) = timeout(TIMEOUT, accept_stream(&peer.conn)).await??;
                    let Message::FileRequest { path } =
                        timeout(TIMEOUT, read_frame(&mut recv)).await??
                    else {
                        return Err::<(), Box<dyn Error>>("FileRequest가 아니다".into());
                    };
                    pending.push((send, path));
                }
                for (mut send, path) in pending {
                    write_frame(
                        &mut send,
                        &Message::FileFound {
                            size: path.len() as u64,
                            hash: *blake3::hash(path.as_bytes()).as_bytes(),
                        },
                    )
                    .await?;
                    send.write_all(path.as_bytes()).await?;
                    send.finish()?;
                }
                Ok(())
            };
            let fetch = |path: &'static str| {
                let handle = peer.handle.clone();
                async move {
                    let mut download = timeout(TIMEOUT, handle.request_file(path))
                        .await??
                        .ok_or("파일이 없다고 답했다")?;
                    let content = timeout(TIMEOUT, download.reader.read_to_end(1024)).await??;
                    assert_eq!(content, path.as_bytes());
                    Ok::<(), Box<dyn Error>>(())
                }
            };

            let (served, first, second) = tokio::join!(serve, fetch("one.txt"), fetch("two.txt"));
            served?;
            first?;
            second
        })
        .await
}
