use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 10_000;
pub const DB_PATH: &str = "./example.db";
pub const IDEN_DIR: &str = "./iden";
/// CLI 요청을 받는 소켓의 파일 이름.
pub const SOCKET_FILE: &str = "rfsync.sock";

pub fn db_path() -> &'static Path {
    Path::new(DB_PATH)
}

/// 데몬이 CLI 요청을 받는 소켓의 경로. 데이터베이스 옆에 둔다. 데이터베이스 하나에 데몬 하나이기 때문이다.
pub fn socket_path(db_path: &Path) -> PathBuf {
    db_path.with_file_name(SOCKET_FILE)
}
