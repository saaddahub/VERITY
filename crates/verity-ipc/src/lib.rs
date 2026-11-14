//! Service protocol: JSON-RPC 2.0 over `\\.\pipe\verity-v1`, each message a
//! 4-byte little-endian length followed by UTF-8 JSON.

pub mod client;
pub mod dispatch;
pub mod framing;
pub mod protocol;
pub mod server;

pub use client::{Client, ClientError};
pub use protocol::*;
