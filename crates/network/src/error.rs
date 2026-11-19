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

    #[error("QUIC TLS 설정 오류: {0}")]
    QuicTls(#[from] quinn::rustls::Error),

    #[error("QUIC 클라이언트 인증서 검증 설정 오류: {0}")]
    QuicClientVerifier(#[from] quinn::rustls::server::VerifierBuilderError),

    #[error("QUIC 초기 암호 스위트 설정 오류: {0}")]
    QuicInitialCipherSuite(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    #[error("QUIC 연결 시작 오류: {0}")]
    QuicConnect(#[from] quinn::ConnectError),

    #[error("QUIC 연결 오류: {0}")]
    QuicConnection(#[from] quinn::ConnectionError),

    #[error("QUIC 수신 엔드포인트가 닫혔습니다")]
    QuicEndpointClosed,

    #[error("메시지 크기 초과: {size}바이트, 최대 {max}바이트")]
    MessageTooLarge { size: usize, max: usize },

    #[error("페어링 인증서용 키 파생 실패")]
    PairingKeyDerivation,

    #[error("상대 인증서의 인증 태그 검증 실패")]
    PeerCertificateAuthentication,
}
