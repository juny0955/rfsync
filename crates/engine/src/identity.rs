use std::path::Path;

use crate::{config::IDEN_DIR, error::EngineError};

pub const SERVER_NAME: &str = "rfsync";

pub struct Identity {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

pub fn load_or_create_identity() -> Result<Identity, EngineError> {
    let iden = match rfsync_fs::load_certificate(Path::new(IDEN_DIR))? {
        Some(iden) => iden,
        None => {
            let (cert, key) = rfsync_network::generate_certificate(SERVER_NAME)?;
            rfsync_fs::save_certificate(Path::new(IDEN_DIR), &cert, &key)?;
            (cert, key)
        }
    };

    Ok(Identity {
        cert: iden.0,
        key: iden.1,
    })
}
