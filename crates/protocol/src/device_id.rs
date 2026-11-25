use serde::{Deserialize, Serialize};

/// 장치를 구분하는 ID. 인증서(DER)의 blake3 해시 앞 8바이트다.
///
/// 같은 인증서에서는 어느 장치에서 계산해도 같은 값이 나온다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceId([u8; 8]);

impl DeviceId {
    pub fn from_cert(cert_der: &[u8]) -> Self {
        let hash = blake3::hash(cert_der);
        let mut id = [0u8; 8];
        id.copy_from_slice(&hash.as_bytes()[..8]);
        Self(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 같은_인증서는_같은_id다() {
        assert_eq!(DeviceId::from_cert(b"cert-a"), DeviceId::from_cert(b"cert-a"));
    }

    #[test]
    fn 다른_인증서는_다른_id다() {
        assert_ne!(DeviceId::from_cert(b"cert-a"), DeviceId::from_cert(b"cert-b"));
    }
}
