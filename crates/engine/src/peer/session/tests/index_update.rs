use super::*;

#[tokio::test]
async fn forwards_an_index_update_from_the_peer_as_an_event() -> Result<(), Box<dyn Error>> {
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
async fn sends_an_index_update_to_the_peer_through_the_handle() -> Result<(), Box<dyn Error>> {
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
