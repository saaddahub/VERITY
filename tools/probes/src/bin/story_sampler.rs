//! Probe: the 4 Hz Frame Story sampler. Does a process started after the
//! query opened show up? (Yes, on the next collection: 2026-11-17.)
//! `cargo run -p verity-probes --bin story_sampler`

use std::time::Duration;

fn main() {
    let mut s = verity_sys::story_sampler::StorySampler::new().expect("sampler");
    std::thread::sleep(Duration::from_millis(300));
    let first = s.sample().expect("sample");
    println!(
        "procs={} gpu_instances={} perf_limit={} on_ac={:?}",
        first.proc_cpu.len(),
        first.gpu_engines.len(),
        first.perf_limit_pct,
        first.on_ac
    );
    let mut child = std::process::Command::new("ping")
        .args(["-n", "4", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let tag = format!("PING:{}", child.id());
    for i in 0..4 {
        std::thread::sleep(Duration::from_millis(250));
        let x = s.sample().expect("sample");
        let seen = x.proc_cpu.iter().any(|(n, _)| n.eq_ignore_ascii_case(&tag));
        println!(
            "sample {i}: new process visible={seen} limit={}",
            x.perf_limit_pct
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}
