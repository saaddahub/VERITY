//! The Verity Windows service (LocalSystem). Registered by `verity service install`.

mod methods;
mod session_mode;
mod storage;
mod story;
mod story_capture;

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use verity_session::machine::Input;
use windows_service::service::{
    PowerEventParam, ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState,
    ServiceStatus, ServiceType, SessionChangeReason,
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

    let rt = tokio::runtime::Runtime::new()?;
    // A kernel trace left running by a crash is stopped before anything else.
    if let Err(e) = verity_sys::etw::stop_by_name(story_capture::KERNEL_SESSION) {
        tracing::warn!("could not clear an old kernel trace: {e}");
    }
    let store = verity_store::Store::open(&data.join("verity.db")).context("open database")?;
    // PresentMon sits beside this binary in the install directory, which
    // standard users cannot write to; its hash is checked before every launch.
    let presentmon =
        std::env::current_exe()?.with_file_name(verity_telemetry::presentmon::FILE_NAME);
    let (service, session_task) = {
        let _enter = rt.enter();
        methods::Service::new(store, presentmon.clone())
    };
    let service = Arc::new(service);
    let session = service.session.clone();

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let rt_handle = rt.handle().clone();
    let status = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        // Suspend: end the session and revert before acknowledging.
        ServiceControl::PowerEvent(PowerEventParam::Suspend) => {
            let s = session.clone();
            rt_handle.block_on(async move { s.input(Input::Suspend).await });
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::PowerEvent(_) => ServiceControlHandlerResult::NoError,
        ServiceControl::SessionChange(p) if p.reason == SessionChangeReason::SessionLogoff => {
            let s = session.clone();
            rt_handle.spawn(async move { s.input(Input::Logoff).await });
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(_) => ServiceControlHandlerResult::NoError,
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let set_state = |state, accept, wait_s| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::NO_ERROR,
            checkpoint: 0,
            wait_hint: Duration::from_secs(wait_s),
            process_id: None,
        })
    };

    set_state(
        ServiceState::Running,
        ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::POWER_EVENT
            | ServiceControlAccept::SESSION_CHANGE,
        5,
    )?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "service started");

    let result = rt
        .block_on(async {
            if verity_telemetry::capture::verify_binary(&presentmon).is_ok() {
                // Clears a trace session orphaned by a crash or a kill.
                verity_telemetry::capture::stop_session(&presentmon).await;
            }
            verity_ipc::server::serve(verity_ipc::PIPE_NAME, service.clone(), stop_rx).await
        })
        .context("IPC server");

    tracing::info!("service stopping");
    // End any session (reverting it) and store what was recorded.
    set_state(ServiceState::StopPending, ServiceControlAccept::empty(), 30)?;
    rt.block_on(async {
        service.session.input(Input::ServiceStopping).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), session_task).await;
    });
    set_state(ServiceState::Stopped, ServiceControlAccept::empty(), 0)?;
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
