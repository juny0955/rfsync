//! `local`과 `peer`를 잇는 coordinator.

mod compare;
mod conflict;
mod coordinator;
mod delete;
mod fetch;
mod resolve;
mod serve;

pub use coordinator::run;
