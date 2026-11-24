use rfsync_engine::{DEFAULT_PORT, db_path, error::EngineError, load_or_create_identity, run};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), EngineError> {
    // 로그는 프로그램의 출력(stdout)과 섞이지 않도록 stderr로 보낸다.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let identity = load_or_create_identity()?;
    let db_path = db_path();
    run(&identity, DEFAULT_PORT, db_path, shutdown_signal()).await?;

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM 핸들러 등록 실패");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
