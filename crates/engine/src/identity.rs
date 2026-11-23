use crate::error::EngineError;

pub const SERVER_NAME: &str = "rfsync";

pub(crate) fn load_or_create_identity() -> Result<(Vec<u8>, Vec<u8>), EngineError> {
    let iden = match rfsync_fs::load_certificate()? {
        Some(iden) => iden,
        None => {
            let (cert, key) = rfsync_network::generate_certificate(SERVER_NAME)?;
            rfsync_fs::save_certificate(&cert, &key)?;
            (cert, key)
        }
    };

    Ok(iden)
}
