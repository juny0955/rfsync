use rcgen::{CertifiedKey, generate_simple_self_signed};

use crate::NetworkError;

pub fn generate_certificate(subject: String) -> Result<(Vec<u8>, Vec<u8>), NetworkError> {
    let subject_alt_names = vec![subject];

    let CertifiedKey { cert, signing_key } = generate_simple_self_signed(subject_alt_names)?;

    let certificate_der = cert.der().to_vec();
    let private_key_der = signing_key.serialize_der();

    Ok((certificate_der, private_key_der))
}
