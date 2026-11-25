use super::*;

#[tokio::test]
async fn 피어의_index_갱신을_이벤트로_전달한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let mut peer = handshake_with_daemon(&fixture, &mut received).await?;

            let update = Message::IndexUpdate {
                entries: vec![entry("new.txt")],
            };
            write_frame(&mut peer.control.0, &update).await?;

            let event = timeout(TIMEOUT, received.recv())
                .await?
                .ok_or("이벤트 통로가 닫혔다")?;
            let PeerEvent::IndexUpdated { entries, .. } = event else {
                return Err("IndexUpdated가 아니다".into());
            };
            assert_eq!(entries, vec![entry("new.txt")]);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 핸들을_통해_피어에게_index_갱신을_보낸다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (events, mut received) = mpsc::channel(8);

    fixture
        .with_daemon_events(events, || async {
            let mut peer = handshake_with_daemon(&fixture, &mut received).await?;

            peer.handle.send_update(vec![entry("changed.txt")]).await?;

            let message = timeout(TIMEOUT, read_frame(&mut peer.control.1)).await??;
            assert_eq!(
                message,
                Message::IndexUpdate {
                    entries: vec![entry("changed.txt")]
                }
            );
            Ok(())
        })
        .await
}
