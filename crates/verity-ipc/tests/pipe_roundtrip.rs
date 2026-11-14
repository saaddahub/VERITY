//! Real named pipe with the production DACL, no admin needed: the test user is
//! an interactive user, which the DACL grants read/write.

use std::sync::Arc;

use serde_json::json;
use verity_core::*;
use verity_ipc::{server, Client, ClientError, Role};

struct Fake;
impl SystemInfoApi for Fake {
    fn system_info(&self) -> Result<SystemInfo, OsError> {
        Ok(SystemInfo {
            windows: WindowsVersion {
                major: 10,
                build: 22631,
                ubr: 0,
                display_version: None,
            },
            cpu: Cpu {
                model: "Fake".into(),
                logical: vec![],
            },
            gpus: vec![],
            displays: vec![],
            power: PowerState::from_raw(1, 128, 255),
            power_scheme: PowerScheme {
                guid: "g".into(),
                name: None,
            },
        })
    }
}

#[tokio::test]
async fn system_info_over_a_real_pipe() {
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
    let info: SystemInfo =
        serde_json::from_value(a.call("system.info", json!({})).await.unwrap()).unwrap();
    assert_eq!(info.windows.build, 22631);
    assert!(matches!(
        b.call("nope", json!({})).await,
        Err(ClientError::Rpc(e)) if e.code == verity_ipc::METHOD_NOT_FOUND
    ));

    stop_tx.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_service_is_reported_plainly() {
    let r = Client::connect(r"\\.\pipe\verity-test-absent", Role::Cli).await;
    assert!(matches!(r, Err(ClientError::ServiceNotRunning)));
}
