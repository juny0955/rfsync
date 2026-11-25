//! CLI와 데몬이 같은 기기 안에서 주고받는 요청·응답과 프레임 형식.
//! 다른 장치와 주고받는 와이어 프로토콜(`rfsync-protocol`)과는 별개다.

#[cfg(feature = "async")]
mod async_frame;
#[cfg(unix)]
mod client;
mod error;
mod frame;
mod message;

#[cfg(feature = "async")]
pub use async_frame::{read_frame_async, write_frame_async};
#[cfg(unix)]
pub use client::{request, request_streaming};
pub use error::IpcError;
pub use frame::{read_frame, write_frame};
pub use message::{Request, Response};

#[cfg(test)]
mod tests;
