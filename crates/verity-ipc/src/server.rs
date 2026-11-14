use std::io;
use std::sync::Arc;

use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::watch;

use crate::dispatch::{Connection, Handler};
use crate::framing::{read_frame, write_frame};

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
                let connected = std::mem::replace(
                    &mut server,
                    verity_sys::pipe::create_server(pipe_name, false)?,
                );
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
        write_frame(&mut pipe, &serde_json::to_vec(&response)?).await?;
    }
    Ok(())
}
