use thiserror::Error;
use tokio::io;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("네트워크 입출력 오류: {0}")]
    Io(#[from] io::Error),

    #[error("SPAKE2 오류: {0}")]
    Spake2(#[from] pakery_spake2::Spake2Error),

    #[error("인증서 오류: {0}")]
    Cert(#[from] rcgen::Error),

    #[error("메시지 크기 초과: {size}바이트, 최대 {max}바이트")]
    MessageTooLarge { size: usize, max: usize },

    #[error("페어링 인증서용 키 파생 실패")]
    PairingKeyDerivation,

    #[error("상대 인증서의 인증 태그 검증 실패")]
    PeerCertificateAuthentication,
}
