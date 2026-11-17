//! Probe (elevated): what the kernel-process provider and a private
//! system-logger kernel session deliver on this machine, and whether kernel
//! flags can be switched on in a running session.
//! `cargo run -p verity-probes --bin etw -- <out.txt>` from an elevated terminal.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use verity_sys::etw::{read, Config, EtwEvent, Provider, Trace};

const KERNEL_PROCESS: u128 = 0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716;
const PERF_INFO: u128 = 0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc;
const DISK_IO: u128 = 0x3d6fa8d4_fe05_11d0_9dda_00c04fd7ba7c;
const PAGE_FAULT: u128 = 0x3d6fa8d3_fe05_11d0_9dda_00c04fd7ba7c;
const IMAGE: u128 = 0x2cb15d1d_5fc1_11d2_abe1_00a0c911f518;
const PROCESS: u128 = 0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c;
const THREAD: u128 = 0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c;

fn short(g: u128) -> String {
    match g {
        KERNEL_PROCESS => "KernelProcess".into(),
        PERF_INFO => "PerfInfo".into(),
        DISK_IO => "DiskIo".into(),
        PAGE_FAULT => "PageFault".into(),
        IMAGE => "Image".into(),
        PROCESS => "Process".into(),
        THREAD => "Thread".into(),
        _ => format!("{g:032x}"),
    }
}

#[derive(Default)]
struct Stats {
    count: u64,
    lens: (usize, usize),
    pid_minus1: u64,
    samples: Vec<String>,
    first_qpc: u64,
}

type Table = Arc<Mutex<BTreeMap<(String, u8, u16, u8), Stats>>>;

fn recorder(table: Table) -> Box<dyn FnMut(&EtwEvent) + Send> {
    Box::new(move |e: &EtwEvent| {
        let mut t = table.lock().unwrap();
        let s = t
            .entry((short(e.provider), e.opcode, e.id, e.version))
            .or_default();
        if s.count == 0 {
            s.lens = (e.data.len(), e.data.len());
            s.first_qpc = e.qpc;
        }
        s.count += 1;
        s.lens.0 = s.lens.0.min(e.data.len());
        s.lens.1 = s.lens.1.max(e.data.len());
        if e.pid == u32::MAX {
            s.pid_minus1 += 1;
        }
        if s.samples.len() < 3 {
            let hex: String = e.data.iter().take(72).map(|b| format!("{b:02x}")).collect();
            let mut extra = String::new();
            if e.provider == KERNEL_PROCESS && (e.id == 1 || e.id == 2) {
                let off = if e.version == 0 { 20 } else { 24 };
                let pid = read::u32_at(e.data, 0);
                let ct = read::u64_at(e.data, 4);
                if e.id == 1 {
                    extra = format!(
                        " pid={pid:?} create={ct:?} name={:?}",
                        read::wstr_at(e.data, off).map(|x| x.0)
                    );
                } else {
                    extra = format!(" pid={pid:?} create={ct:?}");
                }
            }
            if e.provider == IMAGE {
                let n = if e.is_64bit { 56 } else { 36 };
                extra = format!(
                    " base={:x?} size={:x?} pid={:?} name@{n}={:?}",
                    read::ptr_at(e.data, 0, e.is_64bit),
                    read::ptr_at(e.data, if e.is_64bit { 8 } else { 4 }, e.is_64bit),
                    read::u32_at(e.data, if e.is_64bit { 16 } else { 8 }),
                    read::wstr_at(e.data, n).map(|x| x.0)
                );
            }
            if e.provider == PERF_INFO {
                extra = format!(
                    " initial={:?} dur_ticks={:?} routine={:x?}",
                    read::u64_at(e.data, 0),
                    read::u64_at(e.data, 0).map(|i| e.qpc as i64 - i as i64),
                    read::ptr_at(e.data, 8, e.is_64bit)
                );
            }
            s.samples.push(format!(
                "hdr_pid={} tid={} 64={} len={}{extra}\n      {hex}",
                e.pid,
                e.tid,
                e.is_64bit,
                e.data.len()
            ));
        }
    })
}

fn dump(out: &mut String, title: &str, table: &Table) {
    let _ = writeln!(out, "== {title}");
    for ((p, op, id, v), s) in table.lock().unwrap().iter() {
        let _ = writeln!(
            out,
            "{p:<14} op={op:<3} id={id:<3} v={v} n={:<7} len={:?} pid=-1:{} first_qpc={}",
            s.count, s.lens, s.pid_minus1, s.first_qpc
        );
        for x in &s.samples {
            let _ = writeln!(out, "    {x}");
        }
    }
}

fn busy_work() {
    for _ in 0..3 {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "dir", "/s", r"C:\Windows\System32\drivers"])
            .stdout(std::process::Stdio::null())
            .status();
    }
}

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "etw-probe.txt".into());
    let mut out = String::new();
    let _ = writeln!(
        out,
        "qpc_freq={} qpc_now={}",
        verity_sys::process::qpc_freq(),
        verity_sys::process::qpc()
    );

    // 1. Kernel-Process provider on a normal real-time session.
    let procs: Table = Default::default();
    match Trace::start(
        Config {
            name: "VerityProbeProc",
            guid: 0x5e1f0001_0000_4000_8000_000000000001,
            kernel_flags: 0,
            providers: vec![Provider {
                guid: KERNEL_PROCESS,
                keywords: 0x10,
                level: 4,
            }],
        },
        recorder(procs.clone()),
    ) {
        Ok(t) => {
            std::thread::sleep(Duration::from_millis(500));
            busy_work();
            std::thread::sleep(Duration::from_secs(2));
            t.stop();
            dump(&mut out, "kernel-process provider", &procs);
        }
        Err(e) => {
            let _ = writeln!(out, "kernel-process session failed: {e}");
        }
    }

    // 2. System-logger session started with PROCESS only, then updated in place.
    let kern: Table = Default::default();
    let base = 0x1u32; // PROCESS
    let full = base | 0x2 | 0x4 | 0x20 | 0x40 | 0x2000 | 0x100; // THREAD IMAGE DPC ISR HARDFAULT DISKIO
    match Trace::start(
        Config {
            name: "VerityProbeKernel",
            guid: 0x5e1f0002_0000_4000_8000_000000000002,
            kernel_flags: base,
            providers: vec![],
        },
        recorder(kern.clone()),
    ) {
        Ok(t) => {
            std::thread::sleep(Duration::from_secs(2));
            let before: u64 = kern
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| k.0 == "PerfInfo")
                .map(|(_, s)| s.count)
                .sum();
            let upd = t.update_flags(full);
            let _ = writeln!(
                out,
                "update_flags -> {upd:?}; PerfInfo events before update: {before}"
            );
            busy_work();
            let _ = std::fs::read(r"C:\Windows\System32\shell32.dll");
            std::thread::sleep(Duration::from_secs(3));
            let _ = writeln!(out, "events_lost={}", t.events_lost());
            t.stop();
            dump(
                &mut out,
                "system logger (PROCESS, then updated to full)",
                &kern,
            );
            let _ = writeln!(
                out,
                "session still running after stop: {}",
                verity_sys::etw::is_running("VerityProbeKernel")
            );
        }
        Err(e) => {
            let _ = writeln!(out, "kernel session failed: {e}");
        }
    }
    let _ = writeln!(out, "qpc_end={}", verity_sys::process::qpc());
    std::fs::write(&out_path, out).expect("write probe output");
}
