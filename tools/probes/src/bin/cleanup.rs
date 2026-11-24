//! Probe: what each allowlisted Disk Cleanup handler reports on C:, as the
//! current account. Measures only; deletes nothing (docs/14-storage.md).
//! `cargo run -p verity-probes --bin cleanup` (run again elevated to compare).

const HANDLERS: [&str; 16] = [
    "Temporary Files",
    "Thumbnail Cache",
    "D3D Shader Cache",
    "Internet Cache Files",
    "Downloaded Program Files",
    "Update Cleanup",
    "Delivery Optimization Files",
    "Windows Error Reporting Files",
    "System error memory dump files",
    "System error minidump files",
    "Setup Log Files",
    "Temporary Setup Files",
    "Old ChkDsk Files",
    "Windows Upgrade Log Files",
    "Windows Reset Log Files",
    "Feedback Hub Archive log files",
];

fn main() {
    for h in HANDLERS {
        let t = std::time::Instant::now();
        let r = std::thread::spawn(move || verity_sys::cleanup::measure(h, "C:\\"))
            .join()
            .expect("measure thread");
        match r {
            Ok(m) => println!(
                "{h:<32} {:>10.1} MB  {:>5.1} s  \"{}\"",
                m.bytes as f64 / 1_048_576.0,
                t.elapsed().as_secs_f64(),
                m.name
            ),
            Err(e) => println!("{h:<32} error {e}"),
        }
    }
}
