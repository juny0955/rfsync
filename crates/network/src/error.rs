use thiserror::Error;
use tokio::io;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("네트워크 입출력 오류: {0}")]
    Io(#[from] io::Error),

    #[error("메시지 크기 초과: {size}바이트, 최대 {max}바이트")]
    MessageTooLarge { size: usize, max: usize },
}
