//! Load generators for Verity's end-to-end tests. Test tooling only.

use std::io::Write;
use std::process::{Command, ExitCode};
use std::str::FromStr;
use std::time::{Duration, Instant};

const USAGE: &str = "usage:
  stress cpu   --threads <n> --ms <x>
  stress disk  --path <file> --mb <n>
  stress mem   --gb <n> --hold-s <x>
  stress decoy --period-s <n> [--count <n>]
  stress hitch --event <name> --threads <n> --ms <x>   (signal framegen, then load the CPU)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("", &[][..]),
    };
    let result = match cmd {
        "cpu" => opt(rest, "--threads").and_then(|t| opt(rest, "--ms").map(|ms| cpu(t, ms))),
        "disk" => opt::<String>(rest, "--path").and_then(|p| disk(&p, opt(rest, "--mb")?)),
        "mem" => opt(rest, "--gb").and_then(|g| opt(rest, "--hold-s").map(|s| mem(g, s))),
        "decoy" => opt(rest, "--period-s").and_then(|p| decoy(p, opt(rest, "--count").ok())),
        "decoy-child" => Ok(()), // starts and exits doing nothing
        "hitch" => opt::<String>(rest, "--event").and_then(|e| {
            signal(&e)?;
            cpu(opt(rest, "--threads")?, opt(rest, "--ms")?);
            Ok(())
        }),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

fn opt<T: FromStr>(args: &[String], name: &str) -> Result<T, String> {
    let i = args
        .iter()
        .position(|a| a == name)
        .ok_or_else(|| format!("missing {name}\n{USAGE}"))?;
    args.get(i + 1)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("bad or missing value for {name}"))
}

/// Spins `threads` threads flat out for `ms` milliseconds.
fn cpu(threads: usize, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            std::thread::spawn(move || {
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}

/// Writes `mb` MiB to `path`, flushes it to the device, then deletes it.
fn disk(path: &str, mb: usize) -> Result<(), String> {
    let chunk = vec![0xA5u8; 1 << 20];
    let run = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(path)?;
        for _ in 0..mb {
            f.write_all(&chunk)?;
        }
        f.sync_all()
    };
    let result = run().map_err(|e| format!("disk stress on {path}: {e}"));
    let _ = std::fs::remove_file(path);
    result
}

/// Allocates and touches `gb` GiB, then holds it for `hold_s` seconds.
fn mem(gb: f64, hold_s: u64) {
    const CHUNK: usize = 64 << 20;
    let chunks = (gb * (1u64 << 30) as f64 / CHUNK as f64).ceil() as usize;
    let mut held = Vec::with_capacity(chunks);
    for _ in 0..chunks {
        let mut block = vec![0u8; CHUNK];
        // Touch every page so the memory is really committed and resident.
        for page in block.chunks_mut(4096) {
            page[0] = 1;
        }
        held.push(block);
    }
    std::thread::sleep(Duration::from_secs(hold_s));
    drop(held);
}

/// Starts a do-nothing child process every `period_s` seconds: a false-positive
/// control that is active on a schedule but causes no load.
fn decoy(period_s: u64, count: Option<u64>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut done = 0;
    while count.is_none_or(|c| done < c) {
        Command::new(&exe)
            .arg("decoy-child")
            .status()
            .map_err(|e| format!("spawn decoy child: {e}"))?;
        done += 1;
        std::thread::sleep(Duration::from_secs(period_s));
    }
    Ok(())
}

/// Signals framegen's `--hitch-on-event` event, planting a hitch at the moment
/// this process starts: the planted cause of the Frame Story end-to-end test.
fn signal(name: &str) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    let full: Vec<u16> = format!(r"Local\{name}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: `full` is NUL-terminated; the handle is closed below.
    let h = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            windows::core::PCWSTR(full.as_ptr()),
        )
    }
    .map_err(|e| format!("open event {name}: {e}"))?;
    // SAFETY: `h` was just opened with EVENT_MODIFY_STATE.
    let r = unsafe { SetEvent(h) }.map_err(|e| format!("signal {name}: {e}"));
    // SAFETY: `h` is closed once.
    let _ = unsafe { CloseHandle(h) };
    r
}
