use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("스캔 오류: {0}")]
    Scan(#[from] io::Error),

    #[error("파일 감시 오류: {0}")]
    Watch(#[from] notify::Error),

    #[error("장치 인증서 파일 오류: {0}")]
    Certificate(#[source] io::Error),

    #[error("잘못된 경로: {0:?}")]
    InvalidRelativePath(PathBuf),
}
