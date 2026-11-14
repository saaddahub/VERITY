//! Service registration through the Service Control Manager. Needs an elevated terminal.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context};
use windows_service::service::{
    Service, ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
    ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const NAME: &str = "Verity";
const EXE: &str = "verity-service.exe";
const PRESENTMON: &str = verity_telemetry::presentmon::FILE_NAME;

fn manager(access: ServiceManagerAccess) -> anyhow::Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access)
        .context("open the service manager (run this from an elevated terminal)")
}

fn open(access: ServiceAccess) -> anyhow::Result<Service> {
    manager(ServiceManagerAccess::CONNECT)?
        .open_service(NAME, access)
        .context("open the Verity service (is it installed?)")
}

fn wait_for(service: &Service, want: ServiceState) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = service.query_status()?.current_state;
        if state == want {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("service is {state:?}, expected {want:?} after 15 s; see %ProgramData%\\Verity\\logs");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Copies the service binary into `%ProgramFiles%\Verity` (admin-write only)
/// and registers it from there, so a standard user cannot replace what runs as SYSTEM.
pub fn install(exe: Option<PathBuf>) -> anyhow::Result<()> {
    let src = match exe {
        Some(p) => p,
        None => std::env::current_exe()?.with_file_name(EXE),
    };
    ensure!(
        src.is_file(),
        "service binary not found at {} (run `cargo build --workspace` first)",
        src.display()
    );
    let dir = verity_sys::paths::install_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dst = dir.join(EXE);
    std::fs::copy(&src, &dst)
        .with_context(|| format!("copy service binary to {}", dst.display()))?;

    // PresentMon goes beside the service: next to this exe in a release layout,
    // or in tools/presentmon when run from a cargo target directory.
    let from_repo = src
        .with_file_name("../../tools/presentmon")
        .join(PRESENTMON);
    match [src.with_file_name(PRESENTMON), from_repo]
        .iter()
        .find(|p| p.is_file())
    {
        Some(found) => {
            std::fs::copy(found, dir.join(PRESENTMON)).context("copy PresentMon")?;
        }
        None => println!(
            "warning: {PRESENTMON} not found, so `verity record` will not work; \
             see tools/presentmon/README.md, then install again"
        ),
    }

    let info = ServiceInfo {
        name: OsString::from(NAME),
        display_name: OsString::from("Verity"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: dst.clone(),
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .context("create the Verity service (already installed? run `verity service uninstall`)")?;
    service.set_description("Measures game performance and applies reversible, proven changes.")?;
    service.set_delayed_auto_start(true)?;
    println!("installed {} (automatic, delayed start)", dst.display());
    Ok(())
}

pub fn start() -> anyhow::Result<()> {
    let service = open(ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
    if service.query_status()?.current_state != ServiceState::Running {
        service.start::<&str>(&[]).context("start the service")?;
        wait_for(&service, ServiceState::Running)?;
    }
    println!("service running");
    Ok(())
}

pub fn stop() -> anyhow::Result<()> {
    let service = open(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop().context("stop the service")?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    println!("service stopped");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let service = open(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop().context("stop the service")?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    service.delete().context("delete the service")?;
    drop(service);

    // The process may take a moment to exit and release its image file.
    let dir = verity_sys::paths::install_dir();
    let exe = dir.join(EXE);
    for _ in 0..25 {
        if !exe.exists() || std::fs::remove_file(&exe).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(dir.join(PRESENTMON));
    let _ = std::fs::remove_dir(&dir); // only succeeds if empty
    if exe.exists() {
        println!(
            "service removed; could not delete {} yet, remove it manually",
            exe.display()
        );
    } else {
        println!("service removed");
    }
    Ok(())
}
