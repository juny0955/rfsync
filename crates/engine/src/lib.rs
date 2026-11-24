mod config;
pub mod error;
mod local;
mod peer;

pub use config::{DEFAULT_PORT, db_path};
pub use local::{register_root, watch_path, watch_root};
pub use peer::{Identity, connect, listen, load_or_create_identity, run};
