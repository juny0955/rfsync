use std::{
    fs, io,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixStream as StdUnixStream,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use tokio::{net::UnixListener, task::JoinHandle, time::sleep};
use tracing::{debug, info, warn};

use super::{IpcContext, handlers::handle};
use crate::error::EngineError;

/// 연결을 받지 못했을 때 다시 시도하기 전에 쉬는 시간. 파일 디스크립터가 모자란 경우 등에 바쁜 반복을 막는다.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// 소켓을 열고 요청을 처리하는 작업. 버려지면 작업을 멈추고 소켓 파일을 지운다.
pub(crate) struct IpcServer {
    path: PathBuf,
    task: JoinHandle<()>,
}

/// `path`에 소켓을 열고 요청을 받기 시작한다. tokio 런타임 안에서 불러야 한다.
///
/// 이미 같은 경로에 다른 데몬이 떠 있으면 실패한다. 비정상 종료로 남은 소켓 파일은 지우고 다시 연다.
pub(crate) fn start(path: &Path, context: IpcContext) -> Result<IpcServer, EngineError> {
    let listener = bind(path)?;
    info!("CLI 요청을 받습니다: {}", path.display());
    Ok(IpcServer {
        path: path.to_path_buf(),
        task: tokio::spawn(serve(listener, Arc::new(context))),
    })
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.task.abort();
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(
                "소켓 파일을 지우지 못했습니다: {}: {e}",
                self.path.display()
            );
        }
    }
}

fn bind(path: &Path) -> Result<UnixListener, EngineError> {
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            remove_stale_socket(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e.into()),
    };
    // 만든 직후 바로 같은 사용자만 접속하도록 권한을 좁힌다.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// 같은 경로에 남은 소켓 파일이 아무도 받지 않는 것이면 지운다.
/// 소켓이 아닌 파일은 지우지 않고, 누군가 받고 있으면 이미 데몬이 떠 있는 것이다.
fn remove_stale_socket(path: &Path) -> Result<(), EngineError> {
    if !fs::symlink_metadata(path)?.file_type().is_socket() {
        return Err(EngineError::NotASocket(path.to_path_buf()));
    }
    match StdUnixStream::connect(path) {
        Ok(_) => Err(EngineError::DaemonAlreadyRunning(path.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(e.into()),
    }
}

async fn serve(listener: UnixListener, context: Arc<IpcContext>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let context = context.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, &context).await {
                        debug!("CLI 요청을 처리하지 못했습니다: {e}");
                    }
                });
            }
            Err(e) => {
                warn!("CLI 연결을 받지 못했습니다: {e}");
                sleep(ACCEPT_RETRY_DELAY).await;
            }
        }
    }
}
