//! Probe: prints three counter samples and the process list size, to check the
//! PDH paths and the GPU Engine instance-name format on this machine.

use verity_core::telemetry::{CounterApi, ProcessApi};

fn main() {
    match verity_sys::process::WinProcesses.processes() {
        Ok(p) => println!(
            "{} processes; qpc_freq {}",
            p.len(),
            verity_sys::process::qpc_freq()
        ),
        Err(e) => eprintln!("error: {e}"),
    }
    let mut counters = match verity_sys::pdh::PdhCounters::new() {
        Ok(c) => c,
        Err(e) => return eprintln!("error: {e}"),
    };
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match counters.sample() {
            Ok(s) => {
                let mut busy: Vec<_> = s.gpu_engines.iter().filter(|(_, v)| *v > 0.5).collect();
                busy.sort_by(|a, b| b.1.total_cmp(&a.1));
                println!(
                    "cpu {:.1}%  mem {:.0} MB  gpu engines {} (busy: {:?})",
                    s.cpu_total_pct,
                    s.mem_available_mb,
                    s.gpu_engines.len(),
                    &busy[..busy.len().min(3)]
                );
            }
            Err(e) => eprintln!("error: {e}"),
        }
    }
}
