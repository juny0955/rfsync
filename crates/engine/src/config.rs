use std::path::Path;

pub const DEFAULT_PORT: u16 = 10_000;
pub const DB_PATH: &str = "./example.db";
pub const IDEN_DIR: &str = "./iden";

pub fn db_path() -> &'static Path {
    Path::new(DB_PATH)
}
