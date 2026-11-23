use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("직렬화 오류: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),

    #[error("프레임 크기 초과: {size}바이트, 최대 {max}바이트")]
    FrameTooLarge { size: usize, max: usize },
}
