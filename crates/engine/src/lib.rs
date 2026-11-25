mod config;
pub mod error;
#[cfg(unix)]
mod ipc;
mod local;
mod peer;
mod sync;

pub use config::{DEFAULT_PORT, db_path, socket_path};
pub use local::{register_root, watch_root};
pub use peer::{Identity, connect, listen, load_or_create_identity};
pub use rfsync_protocol::DeviceId;
pub use sync::run;
