use std::path::Path;

use rfsync_network::generate_certificate;

use self::store::{load_certificate, save_certificate};
use crate::{config::IDEN_DIR, error::EngineError};

pub const SERVER_NAME: &str = "rfsync";

mod store;

pub struct Identity {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

pub fn load_or_create_identity() -> Result<Identity, EngineError> {
    let iden = match load_certificate(Path::new(IDEN_DIR))? {
        Some(iden) => iden,
        None => {
            let (cert, key) = generate_certificate(SERVER_NAME)?;
            save_certificate(Path::new(IDEN_DIR), &cert, &key)?;
            (cert, key)
        }
    };

    Ok(Identity {
        cert: iden.0,
        key: iden.1,
    })
}
