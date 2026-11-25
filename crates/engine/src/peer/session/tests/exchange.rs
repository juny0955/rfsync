use super::*;

/// 한 방향으로 흐르는 메모리 통로 한 쌍(쓰는 쪽, 읽는 쪽)을 만든다.
fn channel() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(64 * 1024)
}

/// `exchange`를 제한 시간 안에 실행한다. 구현이 잘못돼 서로 기다리면 멈추지 않고 실패하게 한다.
async fn exchange_in_time(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    index: &[Entry],
) -> Result<PeerHandshake, EngineError> {
    timeout(TIMEOUT, exchange(send, recv, index))
        .await
        .expect("제한 시간 안에 끝나야 한다")
}

#[tokio::test]
async fn exchange_gives_each_side_the_others_index() {
    let (mut a_to_b_write, mut a_to_b_read) = channel();
    let (mut b_to_a_write, mut b_to_a_read) = channel();
    let a_index = vec![entry("a-only.txt")];
    let b_index = vec![entry("b-only.txt"), entry("sub/b.txt")];

    let (a_got, b_got) = tokio::try_join!(
        exchange_in_time(&mut a_to_b_write, &mut b_to_a_read, &a_index),
        exchange_in_time(&mut b_to_a_write, &mut a_to_b_read, &b_index),
    )
    .unwrap();

    assert_eq!(a_got.index, b_index);
    assert_eq!(b_got.index, a_index);
    assert_eq!(a_got.version, PROTOCOL_VERSION);
    assert_eq!(b_got.version, PROTOCOL_VERSION);
}

#[tokio::test]
async fn exchange_reports_the_peers_protocol_version_without_failing() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(
        &mut peer_write,
        &Message::Hello {
            version: PROTOCOL_VERSION + 1,
        },
    )
    .await
    .unwrap();
    write_frame(&mut peer_write, &index_message(Vec::new()))
        .await
        .unwrap();

    let peer = exchange_in_time(&mut my_write, &mut my_read, &[])
        .await
        .unwrap();

    assert_eq!(peer.version, PROTOCOL_VERSION + 1);
}

#[tokio::test]
async fn exchange_rejects_a_first_message_that_is_not_hello() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(&mut peer_write, &index_message(Vec::new()))
        .await
        .unwrap();

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(matches!(
        result,
        Err(EngineError::UnexpectedMessage("Hello"))
    ));
}

#[tokio::test]
async fn exchange_rejects_a_second_message_that_is_not_an_index() {
    let (mut my_write, _peer_read) = channel();
    let (mut peer_write, mut my_read) = channel();
    write_frame(&mut peer_write, &hello()).await.unwrap();
    write_frame(&mut peer_write, &hello()).await.unwrap();

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(matches!(
        result,
        Err(EngineError::UnexpectedMessage("Index"))
    ));
}

#[tokio::test]
async fn exchange_fails_when_the_peer_closes_before_sending() {
    let (mut my_write, _peer_read) = channel();
    let (peer_write, mut my_read) = channel();
    drop(peer_write);

    let result = exchange_in_time(&mut my_write, &mut my_read, &[]).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn exchange_fails_instead_of_sending_an_index_over_the_frame_limit() {
    let (mut my_write, _peer_read) = channel();
    let (_peer_write, mut my_read) = channel();
    // 항목 하나가 약 120바이트라 1 MiB 상한을 넘기려면 만 개를 훌쩍 넘겨야 한다.
    let too_many: Vec<Entry> = (0..20_000)
        .map(|i| entry(&format!("dir/{i:0>100}.txt")))
        .collect();

    let result = exchange_in_time(&mut my_write, &mut my_read, &too_many).await;

    assert!(matches!(
        result,
        Err(EngineError::Protocol(
            rfsync_protocol::ProtocolError::FrameTooLarge { .. }
        ))
    ));
}
