use super::*;

#[tokio::test]
async fn 페어링된_기기의_저장된_엔드포인트로_연결을_시도한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let conn = accept_from_daemon(&peer).await?;
            // 데몬이 연결 핸들을 놓았다면 이 시간 안에 연결이 닫힌다.
            sleep(Duration::from_millis(300)).await;
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 연결이_닫히면_다시_연결을_시도한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let peer = fixture.bind_peer(&fixture.peer)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let first = accept_from_daemon(&peer).await?;
            first.close(0u32.into(), b"test");

            // 첫 대기 시간(1초) 뒤에 데몬이 다시 접속해 와야 한다.
            let second = accept_from_daemon(&peer).await?;
            sleep(Duration::from_millis(300)).await;
            assert!(second.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 실패한_시도_뒤에_재시도한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    // 저장된 인증서와 다른 신원으로 응답하는 피어라서 첫 접속은 인증서 검증에 실패한다.
    let peer = fixture.bind_peer(&new_identity()?)?;
    fixture.pair_peer_at(peer.local_addr()?)?;

    fixture
        .with_daemon(|| async {
            let mut rejected = 0;
            let conn = loop {
                match timeout(TIMEOUT, rfsync_network::accept_quic(&peer)).await? {
                    Ok(conn) => break conn,
                    Err(_) => {
                        rejected += 1;
                        // 이제 올바른 신원으로 응답해서 다음 재시도가 성공하게 한다.
                        peer.set_server_config(Some(rfsync_network::server_config(
                            &fixture.peer.cert,
                            &fixture.peer.key,
                            &[&fixture.daemon.cert],
                        )?));
                    }
                }
            };

            assert!(rejected >= 1, "첫 접속이 실패하지 않았다");
            assert!(conn.close_reason().is_none());
            Ok(())
        })
        .await
}

#[tokio::test]
async fn 페어링된_기기가_없어도_시작하고_종료_시_멈춘다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;

    let result = timeout(TIMEOUT, fixture.run(async {})).await?;

    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn 페어링된_기기가_없으면_페어링되지_않은_피어를_거부한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        if let Ok((_endpoint, conn)) = fixture.connect(&fixture.peer).await {
            timeout(TIMEOUT, conn.closed()).await?;
        }
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn 시작_후_페어링된_피어는_알림을_받으면_수락한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let (devices, devices_rx) = mpsc::channel(1);
    let (events, _rx) = mpsc::channel(8);

    let scenario = async {
        // 시작할 때는 아무도 페어링되어 있지 않다. 나중에 저장하고 알리면 받아들인다.
        sleep(Duration::from_millis(200)).await;
        fixture.pair_peer()?;
        devices.send(()).await?;
        sleep(Duration::from_millis(200)).await;

        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        sleep(Duration::from_millis(300)).await;
        assert!(conn.close_reason().is_none());
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run_full(events, devices_rx, async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn 페어링된_피어의_연결은_열어_둔다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        // 데몬이 연결 핸들을 놓았다면 이 시간 안에 연결이 닫힌다.
        sleep(Duration::from_millis(300)).await;
        assert!(conn.close_reason().is_none());
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn 페어링되지_않은_피어를_거부한_뒤에도_계속_수락한다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let unpaired = new_identity()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        // TLS 1.3에서는 클라이언트 쪽 핸드셰이크가 먼저 끝날 수 있으므로,
        // 접속이 성공했더라도 곧 닫히는지로 거부를 확인한다.
        if let Ok((_endpoint, conn)) = fixture.connect(&unpaired).await {
            timeout(TIMEOUT, conn.closed()).await?;
        }

        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        sleep(Duration::from_millis(300)).await;
        assert!(conn.close_reason().is_none());
        let _ = stop.send(());
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}

#[tokio::test]
async fn 종료하면_연결을_닫는다() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.pair_peer()?;
    let (stop, stopped) = oneshot::channel::<()>();

    let scenario = async {
        let (_endpoint, conn) = fixture.connect(&fixture.peer).await?;
        let _ = stop.send(());
        // idle timeout(30초)보다 짧은 시간 안에 닫히므로 종료 처리로 닫힌 것이다.
        timeout(TIMEOUT, conn.closed()).await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let (daemon, scenario) = timeout(TIMEOUT, async {
        tokio::join!(
            fixture.run(async {
                let _ = stopped.await;
            }),
            scenario
        )
    })
    .await?;

    daemon?;
    scenario?;
    Ok(())
}
