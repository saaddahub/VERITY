use std::io;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::net::windows::named_pipe::NamedPipeClient;

use crate::framing::{read_frame, write_frame};
use crate::protocol::*;

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the Verity service is not running (start it with `verity service start`)")]
    ServiceNotRunning,
    #[error("pipe I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("service returned error {}: {}", .0.code, .0.message)]
    Rpc(RpcError),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub struct Client {
    pipe: NamedPipeClient,
    next_id: u64,
}

impl Client {
    /// Connects to `pipe_name` and performs the `hello` handshake.
    pub async fn connect(pipe_name: &str, role: Role) -> Result<Self, ClientError> {
        let mut attempts = 0;
        let pipe = loop {
            match verity_sys::pipe::open_client(pipe_name) {
                Ok(p) => break p,
                Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                    return Err(ClientError::ServiceNotRunning)
                }
                // All instances busy: the server creates a new one right after each connect.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 40 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e.into()),
            }
        };
        let mut client = Self { pipe, next_id: 1 };
        client
            .call(
                "hello",
                json!({ "protocol": PROTOCOL_VERSION, "role": role }),
            )
            .await?;
        Ok(client)
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::to_vec(&Request::new(id, method, params))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        write_frame(&mut self.pipe, &req).await?;
        let raw = read_frame(&mut self.pipe)
            .await?
            .ok_or_else(|| ClientError::Protocol("service closed the connection".into()))?;
        let resp: Response =
            serde_json::from_slice(&raw).map_err(|e| ClientError::Protocol(e.to_string()))?;
        if resp.id != Some(id) {
            return Err(ClientError::Protocol(format!(
                "response id {:?} does not match request id {id}",
                resp.id
            )));
        }
        match (resp.result, resp.error) {
            (_, Some(e)) => Err(ClientError::Rpc(e)),
            (Some(v), None) => Ok(v),
            (None, None) => Ok(Value::Null),
        }
    }
}
