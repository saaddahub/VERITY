use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::watch;

use crate::dispatch::{Connection, Handler};
use crate::framing::{read_frame, write_frame};
use crate::protocol::{Response, INTERNAL_ERROR, MAX_FRAME};

/// Accepts clients on `pipe_name` until `stop` flips to true.
pub async fn serve<H: Handler>(
    pipe_name: &str,
    handler: Arc<H>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut server = verity_sys::pipe::create_server(pipe_name, true)?;
    tracing::info!(pipe = pipe_name, "listening");
    loop {
        tokio::select! {
            r = server.connect() => {
                // A failure to open the next instance is retried, not fatal: it
                // must not take the whole service down.
                let next = loop {
                    match verity_sys::pipe::create_server(pipe_name, false) {
                        Ok(s) => break s,
                        Err(e) => {
                            tracing::warn!("could not open the next pipe instance: {e}; retrying");
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                                _ = stop.changed() => return Ok(()),
                            }
                        }
                    }
                };
                let connected = std::mem::replace(&mut server, next);
                match r {
                    Ok(()) => {
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle(connected, handler).await {
                                tracing::warn!("client connection ended with error: {e}");
                            }
                        });
                    }
                    Err(e) => tracing::warn!("pipe connect failed: {e}"),
                }
            }
            _ = stop.changed() => return Ok(()),
        }
    }
}

async fn handle<H: Handler>(mut pipe: NamedPipeServer, handler: Arc<H>) -> io::Result<()> {
    let mut conn = Connection::default();
    while let Some(frame) = read_frame(&mut pipe).await? {
        let response = conn.handle(&frame, handler.as_ref()).await;
        write_frame(&mut pipe, &encode(&response)?).await?;
    }
    Ok(())
}

/// The response as bytes. One too large to send becomes an error the client
/// can show, instead of a dropped connection.
fn encode(response: &Response) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(response)?;
    if bytes.len() <= MAX_FRAME {
        return Ok(bytes);
    }
    tracing::warn!(bytes = bytes.len(), "response too large to send");
    let err = Response::err(
        response.id,
        INTERNAL_ERROR,
        format!(
            "the reply is {} KB, over the {} KB limit",
            bytes.len() / 1024,
            MAX_FRAME / 1024
        ),
    );
    Ok(serde_json::to_vec(&err)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oversized_replies_become_errors() {
        let small = Response::ok(3, json!("ok"));
        assert_eq!(encode(&small).unwrap(), serde_json::to_vec(&small).unwrap());
        let big = Response::ok(4, json!("x".repeat(MAX_FRAME)));
        let back: Response = serde_json::from_slice(&encode(&big).unwrap()).unwrap();
        assert_eq!(back.id, Some(4));
        assert_eq!(back.error.unwrap().code, INTERNAL_ERROR);
    }
}
