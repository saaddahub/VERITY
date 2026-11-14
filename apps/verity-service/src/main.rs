//! The Verity Windows service (LocalSystem). Registered by `verity service install`.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

const SERVICE_NAME: &str = "Verity";

define_windows_service!(ffi_service_main, service_main);

fn main() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("not started by the service manager; use `verity service start`")
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run() {
        tracing::error!("service stopped with error: {e:#}");
    }
}

fn run() -> anyhow::Result<()> {
    let data = verity_sys::paths::data_dir();
    verity_sys::paths::ensure_data_dir(&data).context("prepare data directory")?;
    let _log_guard = init_logging(&data.join("logs"))?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let status = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let set_state = |state, accept| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::NO_ERROR,
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })
    };

    set_state(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    )?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "service started");

    let result = tokio::runtime::Runtime::new()?
        .block_on(verity_ipc::server::serve(
            verity_ipc::PIPE_NAME,
            Arc::new(verity_sys::WinSystemInfo),
            stop_rx,
        ))
        .context("IPC server");

    tracing::info!("service stopping");
    set_state(ServiceState::Stopped, ServiceControlAccept::empty())?;
    result
}

/// Daily-rotated logs, last 7 days kept.
fn init_logging(dir: &Path) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("service")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir)
        .context("open log file")?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(guard)
}
