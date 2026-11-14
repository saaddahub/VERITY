//! Probe: prints what `verity-sys` reads on this machine, with no service involved.
//! Run it as yourself and (later, from the service) compare the `displays` list.

use verity_core::SystemInfoApi;

fn main() {
    match verity_sys::WinSystemInfo.system_info() {
        Ok(info) => match serde_json::to_string_pretty(&info) {
            Ok(json) => println!("{json}"),
            Err(e) => eprintln!("error: {e}"),
        },
        Err(e) => eprintln!("error: {e}"),
    }
}
