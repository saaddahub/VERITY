//! Real named pipe with the production DACL, no admin needed: the test user is
//! an interactive user, which the DACL grants read/write.

use std::sync::Arc;

use serde_json::{json, Value};
use verity_ipc::dispatch::Handler;
use verity_ipc::{server, Client, ClientError, Role, RpcError, METHOD_NOT_FOUND};

struct Fake;
impl Handler for Fake {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "echo" => Ok(params),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method {other}"),
            )),
        }
    }
}

#[tokio::test]
async fn calls_over_a_real_pipe() {
    let name = format!(r"\\.\pipe\verity-test-{}", std::process::id());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let server = {
        let name = name.clone();
        tokio::spawn(async move { server::serve(&name, Arc::new(Fake), stop_rx).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Two clients at once: the server must keep a fresh instance listening.
    let mut a = Client::connect(&name, Role::Cli).await.unwrap();
    let mut b = Client::connect(&name, Role::Ui).await.unwrap();
    let sent = json!({"build": 22631});
    assert_eq!(a.call("echo", sent.clone()).await.unwrap(), sent);
    assert!(matches!(
        b.call("nope", json!({})).await,
        Err(ClientError::Rpc(e)) if e.code == METHOD_NOT_FOUND
    ));

    stop_tx.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_service_is_reported_plainly() {
    let r = Client::connect(r"\\.\pipe\verity-test-absent", Role::Cli).await;
    assert!(matches!(r, Err(ClientError::ServiceNotRunning)));
}
