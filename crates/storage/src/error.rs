use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite 오류: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("스키마 버전이 없지만 비어있지 않은 DB입니다.")]
    UnversionedDatabase,

    #[error("지원하지 않는 스키마 버전: {0}")]
    UnsupportedSchemaVersion(i64),

    #[error("Root를 찾을 수 없습니다: {0}")]
    RootNotFound(i64),
}
