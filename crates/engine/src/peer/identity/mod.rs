use std::path::Path;

use rfsync_network::generate_certificate;
use rfsync_protocol::DeviceId;

use self::store::{load_certificate, save_certificate};
use crate::{config::IDEN_DIR, error::EngineError};

pub const SERVER_NAME: &str = "rfsync";

mod store;

pub struct Identity {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

impl Identity {
    /// 내 인증서에서 계산한 장치 ID. 상대가 나를 부르는 ID와 같다.
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_cert(&self.cert)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 장치_id는_내_인증서로_계산한_값이다() {
        let identity = Identity {
            cert: b"cert".to_vec(),
            key: b"key".to_vec(),
        };

        assert_eq!(identity.device_id(), DeviceId::from_cert(b"cert"));
    }
}
