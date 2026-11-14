use std::io;
use std::sync::Arc;

use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::watch;
use verity_core::SystemInfoApi;

use crate::dispatch::Connection;
use crate::framing::{read_frame, write_frame};

/// Accepts clients on `pipe_name` until `stop` flips to true.
pub async fn serve(
    pipe_name: &str,
    api: Arc<dyn SystemInfoApi>,
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
                        let api = api.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle(connected, api).await {
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

async fn handle(mut pipe: NamedPipeServer, api: Arc<dyn SystemInfoApi>) -> io::Result<()> {
    let mut conn = Connection::default();
    while let Some(frame) = read_frame(&mut pipe).await? {
        let response = conn.handle(&frame, api.as_ref());
        write_frame(&mut pipe, &serde_json::to_vec(&response)?).await?;
    }
    Ok(())
}
