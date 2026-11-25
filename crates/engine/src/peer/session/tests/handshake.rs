use super::*;

/// 루트 하나를 등록한 DB와 그 목록이 만드는 기대 메시지를 준비한다.
fn register_root_with_files(
    fixture: &Fixture,
) -> Result<(tempfile::TempDir, Message), Box<dyn Error>> {
    let root_dir = tempfile::tempdir()?;
    std::fs::write(root_dir.path().join("a.txt"), "hello")?;
    std::fs::create_dir(root_dir.path().join("sub"))?;
    std::fs::write(root_dir.path().join("sub/b.txt"), "world")?;
    crate::register_root(
        root_dir.path(),
        &fixture.db_path,
        crate::DeviceId::from_cert(b"test"),
    )?;
    let expected = index_message(load_index(&fixture.db_path)?);
    Ok((root_dir, expected))
}

#[tokio::test]
async fn 데몬에_연결한_피어에게_hello_다음_index를_보낸다() -> Result<(), Box<dyn Error>>
{
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (_root, expected) = register_root_with_files(&fixture)?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;

            let (first, second) = read_handshake(&mut recv).await?;

            assert_eq!(first, hello());
            assert_eq!(second, expected);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 데몬이_연결한_피어에게_hello_다음_index를_보낸다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;
    let (_root, expected) = register_root_with_files(&fixture)?;

    fixture
        .with_daemon(|| async {
            let conn = accept_from_daemon(&peer).await?;
            let (mut send, mut recv) = timeout(TIMEOUT, accept_stream(&conn)).await??;
            write_handshake(&mut send).await?;

            let (first, second) = read_handshake(&mut recv).await?;

            assert_eq!(first, hello());
            assert_eq!(second, expected);
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 등록된_루트가_없으면_빈_index를_보낸다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;

            let (_, second) = read_handshake(&mut recv).await?;

            assert_eq!(second, index_message(Vec::new()));
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 핸드셰이크_후에도_연결을_열어_둔다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, mut recv) = open_stream(&conn).await?;
            write_handshake(&mut send).await?;
            read_handshake(&mut recv).await?;

            // 핸드셰이크가 끝나도 데몬이 연결을 닫으면 안 된다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 피어가_예상치_못한_메시지를_보내도_연결을_열어_둔다()
-> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
            let (mut send, _recv) = open_stream(&conn).await?;
            // Hello가 와야 할 자리에 목록을 보내 핸드셰이크를 실패시킨다.
            write_frame(&mut send, &index_message(Vec::new())).await?;

            // 핸드셰이크가 실패해도 연결 자체는 닫지 않는다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 루트가_등록되면_시작하고_서비스한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let root_dir = tempfile::tempdir()?;
    std::fs::write(root_dir.path().join("a.txt"), "hello")?;
    crate::register_root(
        root_dir.path(),
        &fixture.db_path,
        crate::DeviceId::from_cert(b"test"),
    )?;

    fixture
        .with_daemon(|| async {
            let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;

            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}
