mod cert;
pub mod error;
mod pairing;
mod quic;

pub use cert::generate_certificate;
pub use error::NetworkError;
pub use pairing::{
    PairingListener, bind_pairing, connect_pairing, generate_pairing_code, listen_pairing,
};
pub use quic::{
    accept_quic, accept_stream, bind_quic, client_config, connect_quic, open_stream,
    peer_certificate, server_config,
};

pub use quinn::{
    ClientConfig, Connection as QuicConnection, Endpoint, RecvStream as QuicRecvStream,
    SendStream as QuicSendStream, ServerConfig,
};
