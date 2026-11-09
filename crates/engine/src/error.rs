use std::{io, num::TryFromIntError, path::PathBuf, time::SystemTimeError};

use rfsync_storage::error::StorageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("이미 등록된 root path 입니다: {}", .0.display())]
    RootAlreadyExists(PathBuf),
    #[error("root 경로를 UTF-8 문자열로 표현할 수 없습니다: {}", .0.display())]
    InvalidRootPath(PathBuf),
    #[error("지원하지 않는 항목 경로입니다: {}", .0.display())]
    InvalidEntryPath(PathBuf),
    #[error("부모 폴더를 찾을 수 없습니다: {}", .0.display())]
    ParentFolderNotFound(PathBuf),
    #[error("등록되지 않은 root입니다: {0}")]
    RootNotFound(i64),
    #[error("이미 스냅샷이 존재합니다: {0}")]
    SnapshotAlreadyExists(i64),
    #[error("입출력 오류: {0}")]
    Io(#[from] io::Error),
    #[error("저장소 오류: {0}")]
    Storage(#[from] StorageError),
    #[error("파일 시간 오류: {0}")]
    Time(#[from] SystemTimeError),
    #[error("정수 변환 오류: {0}")]
    IntegerConversion(#[from] TryFromIntError),
}
