use std::path::Path;

use rfsync_engine::{DEFAULT_PORT, error::EngineError, load_or_create_identity, run};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), EngineError> {
    let identity = load_or_create_identity()?;
    let db_path = Path::new("./example.db");
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
