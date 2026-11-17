//! Kernel tracing for Standard captures (docs/03-telemetry.md §4) and the
//! process events Session Mode uses (docs/06-session-mode.md).
//!
//! Payload layouts were read on the dev laptop (Windows 11 25H2, build 26200)
//! by `tools/probes/src/bin/etw.rs`; see docs/notes/decisions.md. Decoding is
//! pure and bounds-checked: a short or unexpected payload yields nothing.

use std::collections::HashMap;

use verity_core::telemetry::SysEvent;
use verity_sys::etw::{read, EtwEvent};

pub const KERNEL_PROCESS: u128 = 0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716;
/// `WINEVENT_KEYWORD_PROCESS` of Microsoft-Windows-Kernel-Process.
pub const KERNEL_PROCESS_KEYWORD: u64 = 0x10;
pub const PERF_INFO: u128 = 0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc;
pub const DISK_IO: u128 = 0x3d6fa8d4_fe05_11d0_9dda_00c04fd7ba7c;
pub const PAGE_FAULT: u128 = 0x3d6fa8d3_fe05_11d0_9dda_00c04fd7ba7c;
pub const IMAGE: u128 = 0x2cb15d1d_5fc1_11d2_abe1_00a0c911f518;
pub const THREAD: u128 = 0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c;

/// Kernel flags for a Standard capture: THREAD (thread→process map),
/// IMAGE_LOAD (driver address map), DPC, INTERRUPT, MEMORY_HARD_FAULTS, DISK_IO.
pub const STANDARD_FLAGS: u32 = 0x2 | 0x4 | 0x20 | 0x40 | 0x2000 | 0x100;

/// A process start or stop from Microsoft-Windows-Kernel-Process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcEvent {
    Start {
        pid: u32,
        /// FILETIME.
        create_time: u64,
        /// NT path, e.g. `\Device\HarddiskVolume3\Games\game.exe`.
        image: String,
    },
    Stop {
        pid: u32,
        create_time: u64,
    },
}

/// FILETIMEs after 2015 and before 2100: rejects a misread offset.
fn plausible_filetime(t: u64) -> bool {
    (0x01D0_0000_0000_0000..0x0230_0000_0000_0000).contains(&t)
}

/// Decodes ProcessStart (id 1) and ProcessStop (id 2).
///
/// Probed layout (v4 start, v2 stop): ProcessID u32, ProcessSequenceNumber u64,
/// CreateTime u64, ... Older versions lack the sequence number, so CreateTime
/// is at 4. The image name follows a variable-length SID in v4, so it is found
/// by its `\` prefix rather than a fixed offset.
pub fn decode_process(e: &EtwEvent) -> Option<ProcEvent> {
    if e.provider != KERNEL_PROCESS || !(e.id == 1 || e.id == 2) {
        return None;
    }
    let d = e.data;
    let pid = read::u32_at(d, 0)?;
    let create_time = [12, 4]
        .into_iter()
        .filter_map(|off| read::u64_at(d, off))
        .find(|&t| plausible_filetime(t))?;
    if e.id == 2 {
        return Some(ProcEvent::Stop { pid, create_time });
    }
    // The image name is the first UTF-16 string starting with '\' after the fixed fields.
    let mut off = 20;
    while off + 4 <= d.len() {
        if d[off] == b'\\'
            && d[off + 1] == 0
            && d[off + 2].is_ascii_alphanumeric()
            && d[off + 3] == 0
        {
            let (image, _) = read::wstr_at(d, off)?;
            return Some(ProcEvent::Start {
                pid,
                create_time,
                image,
            });
        }
        off += 2;
    }
    None
}

/// File name of a path in any form (`\Device\...`, `\SystemRoot\...`, `C:\...`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Thresholds from docs/07-frame-story.md step 2.
const DPC_SINGLE_US: f64 = 500.0;
const DPC_BUCKET_MS: u64 = 10;
const DPC_BUCKET_TOTAL_US: f64 = 2000.0;
const FAULT_WINDOW_MS: u64 = 100;
const FAULT_COUNT: u32 = 5;
const DISK_SLOW_MS: f64 = 50.0;
const DISK_WINDOW_MS: u64 = 250;
const DISK_BURST_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Default)]
struct Bucket {
    start: u64,
    total_ticks: u64,
    count: u32,
}

/// Turns kernel events into Frame Story events as they arrive. Holds only
/// small aggregates, so a long capture does not grow memory with DPC volume.
pub struct Collector {
    freq: u64,
    game_pid: u32,
    /// Kernel modules: (base, end, file name), sorted by base.
    modules: Vec<(u64, u64, String)>,
    modules_sorted: bool,
    threads: HashMap<u32, u32>,
    names: HashMap<u32, String>,
    dpc: HashMap<(String, bool), Bucket>,
    faults: HashMap<u32, (u64, u32)>,
    disk: HashMap<u32, (u64, u64)>,
    pub events: Vec<SysEvent>,
    /// DPC/ISR routines that matched no module.
    pub unattributed: u64,
}

impl Collector {
    /// `names` maps pid to image file name at capture start; process starts
    /// during the capture are added with [`Collector::process_started`].
    pub fn new(freq: u64, game_pid: u32, names: HashMap<u32, String>) -> Self {
        Self {
            freq,
            game_pid,
            modules: Vec::new(),
            modules_sorted: true,
            threads: HashMap::new(),
            names,
            dpc: HashMap::new(),
            faults: HashMap::new(),
            disk: HashMap::new(),
            events: Vec::new(),
            unattributed: 0,
        }
    }

    fn ticks_ms(&self, ms: u64) -> u64 {
        ms * self.freq / 1000
    }

    fn us(&self, ticks: u64) -> f64 {
        ticks as f64 * 1e6 / self.freq as f64
    }

    pub fn process_started(&mut self, pid: u32, image: &str) {
        self.names.insert(pid, file_name(image).to_string());
    }

    fn process_name(&self, pid: u32) -> String {
        self.names
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| format!("pid {pid}"))
    }

    fn driver(&mut self, addr: u64) -> Option<String> {
        if !self.modules_sorted {
            self.modules.sort_by_key(|m| m.0);
            self.modules_sorted = true;
        }
        let i = self.modules.partition_point(|m| m.0 <= addr);
        let m = self.modules.get(i.checked_sub(1)?)?;
        (addr < m.1).then(|| m.2.clone())
    }

    pub fn feed(&mut self, e: &EtwEvent) {
        match e.provider {
            IMAGE => self.on_image(e),
            THREAD => self.on_thread(e),
            PERF_INFO => self.on_perf(e),
            PAGE_FAULT if e.opcode == 32 => self.on_hard_fault(e),
            DISK_IO if e.opcode == 10 || e.opcode == 11 => self.on_disk(e),
            _ => {}
        }
    }

    fn on_image(&mut self, e: &EtwEvent) {
        // Load (10) and rundown (3). Kernel modules carry ProcessId 0.
        if !(e.opcode == 10 || e.opcode == 3) {
            return;
        }
        let w = if e.is_64bit { 8 } else { 4 };
        let (Some(base), Some(size), Some(pid)) = (
            read::ptr_at(e.data, 0, e.is_64bit),
            read::ptr_at(e.data, w, e.is_64bit),
            read::u32_at(e.data, 2 * w),
        ) else {
            return;
        };
        if pid != 0 {
            return;
        }
        let name_off = if e.is_64bit { 56 } else { 36 };
        if let Some((path, _)) = read::wstr_at(e.data, name_off) {
            self.modules.push((
                base,
                base.saturating_add(size),
                file_name(&path).to_string(),
            ));
            self.modules_sorted = false;
        }
    }

    fn on_thread(&mut self, e: &EtwEvent) {
        let (Some(pid), Some(tid)) = (read::u32_at(e.data, 0), read::u32_at(e.data, 4)) else {
            return;
        };
        match e.opcode {
            1 | 3 => {
                self.threads.insert(tid, pid);
            }
            // Thread end: keep the mapping briefly-late events still need; tids
            // are reused, and a start overwrites it.
            _ => {}
        }
    }

    fn on_perf(&mut self, e: &EtwEvent) {
        let kind = match e.opcode {
            66 | 68 | 69 => "dpc",
            50 | 67 => "isr",
            _ => return,
        };
        let (Some(initial), Some(routine)) =
            (read::u64_at(e.data, 0), read::ptr_at(e.data, 8, e.is_64bit))
        else {
            return;
        };
        let dur = e.qpc.saturating_sub(initial);
        let Some(driver) = self.driver(routine) else {
            self.unattributed += 1;
            return;
        };
        let us = self.us(dur);
        if us > DPC_SINGLE_US {
            self.events.push(SysEvent {
                qpc: initial,
                duration_ticks: dur,
                kind: kind.into(),
                subject: driver.clone(),
                magnitude: us,
            });
        }
        let span = self.ticks_ms(DPC_BUCKET_MS);
        let bucket_start = initial - initial % span.max(1);
        let key = (driver, kind == "isr");
        let flush = match self.dpc.get(&key) {
            Some(b) => b.start != bucket_start,
            None => false,
        };
        if flush {
            if let Some(b) = self.dpc.remove(&key) {
                self.emit_bucket(&key, b);
            }
        }
        let b = self.dpc.entry(key).or_insert_with(|| Bucket {
            start: bucket_start,
            ..Default::default()
        });
        b.total_ticks += dur;
        b.count += 1;
    }

    fn emit_bucket(&mut self, key: &(String, bool), b: Bucket) {
        let us = self.us(b.total_ticks);
        if us > DPC_BUCKET_TOTAL_US {
            self.events.push(SysEvent {
                qpc: b.start,
                duration_ticks: self.ticks_ms(DPC_BUCKET_MS),
                kind: if key.1 { "isr" } else { "dpc" }.into(),
                subject: key.0.clone(),
                magnitude: us,
            });
        }
    }

    fn on_hard_fault(&mut self, e: &EtwEvent) {
        // HardFault: InitialTime u64, ReadOffset u64, VirtualAddress ptr,
        // FileObject ptr, TThreadId u32, ByteCount u32.
        let w = if e.is_64bit { 8 } else { 4 };
        let Some(tid) = read::u32_at(e.data, 16 + 2 * w) else {
            return;
        };
        let Some(&pid) = self.threads.get(&tid) else {
            return;
        };
        let span = self.ticks_ms(FAULT_WINDOW_MS);
        let window = e.qpc - e.qpc % span.max(1);
        let entry = self.faults.entry(pid).or_insert((window, 0));
        if entry.0 != window {
            *entry = (window, 0);
        }
        entry.1 += 1;
        if entry.1 == FAULT_COUNT {
            let subject = self.process_name(pid);
            self.events.push(SysEvent {
                qpc: window,
                duration_ticks: span,
                kind: "hard_fault".into(),
                subject,
                magnitude: FAULT_COUNT as f64,
            });
        } else if entry.1 > FAULT_COUNT {
            // Count the rest into the event already emitted for this window.
            if let Some(ev) = self
                .events
                .iter_mut()
                .rev()
                .find(|x| x.kind == "hard_fault" && x.qpc == window)
            {
                ev.magnitude += 1.0;
            }
        }
    }

    fn on_disk(&mut self, e: &EtwEvent) {
        // DiskIo v3: DiskNumber, IrpFlags, TransferSize, Reserved (u32 each),
        // ByteOffset u64, FileObject ptr, Irp ptr, HighResResponseTime u64,
        // IssuingThreadId u32. Logged at completion.
        let w = if e.is_64bit { 8 } else { 4 };
        let (Some(size), Some(resp), Some(tid)) = (
            read::u32_at(e.data, 8),
            read::u64_at(e.data, 24 + 2 * w),
            read::u32_at(e.data, 32 + 2 * w),
        ) else {
            return;
        };
        let pid = self.threads.get(&tid).copied();
        let subject = pid.map_or_else(|| "System".to_string(), |p| self.process_name(p));
        let start = e.qpc.saturating_sub(resp);
        let ms = resp as f64 * 1000.0 / self.freq as f64;
        if ms > DISK_SLOW_MS {
            self.events.push(SysEvent {
                qpc: start,
                duration_ticks: resp,
                kind: "disk_io".into(),
                subject: subject.clone(),
                magnitude: ms,
            });
        }
        let Some(pid) = pid.filter(|&p| p != self.game_pid) else {
            return;
        };
        let span = self.ticks_ms(DISK_WINDOW_MS);
        let window = start - start % span.max(1);
        let entry = self.disk.entry(pid).or_insert((window, 0));
        if entry.0 != window {
            *entry = (window, 0);
        }
        let before = entry.1;
        entry.1 += size as u64;
        if before <= DISK_BURST_BYTES && entry.1 > DISK_BURST_BYTES {
            self.events.push(SysEvent {
                qpc: window,
                duration_ticks: span,
                kind: "disk_io".into(),
                subject,
                magnitude: DISK_BURST_BYTES as f64 / 1_048_576.0,
            });
        }
    }

    /// Emits the DPC/ISR buckets still open; call once at capture end.
    pub fn finish(mut self) -> (Vec<SysEvent>, u64) {
        let open: Vec<_> = self.dpc.drain().collect();
        for (k, b) in open {
            self.emit_bucket(&k, b);
        }
        self.events.sort_by_key(|e| e.qpc);
        (self.events, self.unattributed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u64 = 10_000_000;

    fn ev(provider: u128, opcode: u8, qpc: u64, data: &[u8]) -> EtwEvent<'_> {
        EtwEvent {
            provider,
            id: 0,
            version: 2,
            opcode,
            pid: u32::MAX,
            tid: u32::MAX,
            qpc,
            data,
            is_64bit: true,
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn utf16z(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain(Some(0))
            .flat_map(|u| u.to_le_bytes())
            .collect()
    }

    /// ProcessStart v4 as captured on the dev laptop: fixed fields, a
    /// mandatory-label SID, then the image name.
    #[test]
    fn decodes_probed_process_start_and_stop() {
        let mut start = hex(
            "5c7300006e61010000000000e6e998a26476dd01c80b00006c610100000000000d000000000000000200000001000000010100000000001000300000",
        );
        start.extend(utf16z(r"\Device\HarddiskVolume3\Windows\System32\cmd.exe"));
        start.extend([0u8; 16]);
        let e = EtwEvent {
            id: 1,
            version: 4,
            ..ev(KERNEL_PROCESS, 1, 0, &start)
        };
        assert_eq!(
            decode_process(&e),
            Some(ProcEvent::Start {
                pid: 29532,
                create_time: 0x01dd_7664_a298_e9e6,
                image: r"\Device\HarddiskVolume3\Windows\System32\cmd.exe".into(),
            })
        );
        let stop = hex("5c7300006e61010000000000e6e998a26476dd01fb9da2a26476dd01");
        let e = EtwEvent {
            id: 2,
            ..ev(KERNEL_PROCESS, 2, 0, &stop)
        };
        assert_eq!(
            decode_process(&e),
            Some(ProcEvent::Stop {
                pid: 29532,
                create_time: 0x01dd_7664_a298_e9e6
            })
        );
        // Older layout: CreateTime at 4, name at 24.
        let mut old = Vec::new();
        old.extend(77u32.to_le_bytes());
        old.extend(0x01dc_0000_0000_0001u64.to_le_bytes());
        old.extend([0u8; 12]);
        old.extend(utf16z(r"\Device\X\a.exe"));
        let e = EtwEvent {
            id: 1,
            version: 1,
            ..ev(KERNEL_PROCESS, 1, 0, &old)
        };
        assert!(matches!(
            decode_process(&e),
            Some(ProcEvent::Start {
                pid: 77,
                create_time: 0x01dc_0000_0000_0001,
                ..
            })
        ));
        // Truncated payloads decode to nothing.
        for n in [0, 3, 11, 20] {
            let e = EtwEvent {
                id: 1,
                ..ev(KERNEL_PROCESS, 1, 0, &start[..n])
            };
            assert_eq!(decode_process(&e), None);
        }
    }

    fn image(base: u64, size: u64, pid: u32, path: &str) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(base.to_le_bytes());
        d.extend(size.to_le_bytes());
        d.extend(pid.to_le_bytes());
        d.extend([0u8; 36]);
        assert_eq!(d.len(), 56);
        d.extend(utf16z(path));
        d
    }

    fn dpc(initial: u64, routine: u64) -> Vec<u8> {
        let mut d = initial.to_le_bytes().to_vec();
        d.extend(routine.to_le_bytes());
        d
    }

    fn collector() -> Collector {
        let mut c = Collector::new(FREQ, 100, HashMap::from([(200, "stress.exe".into())]));
        let img = image(
            0xfffff800_10000000,
            0x10000,
            0,
            r"\SystemRoot\system32\drivers\Netwtw10.sys",
        );
        c.feed(&ev(IMAGE, 3, 0, &img));
        let user = image(0x7ff6_0000_0000, 0x1000, 55, r"\Device\X\user.dll");
        c.feed(&ev(IMAGE, 10, 0, &user));
        let mut t = Vec::new();
        t.extend(200u32.to_le_bytes());
        t.extend(7u32.to_le_bytes());
        c.feed(&ev(THREAD, 3, 0, &t));
        c
    }

    #[test]
    fn dpc_singles_and_buckets_are_attributed_to_drivers() {
        let mut c = collector();
        let netw = 0xfffff800_10000500;
        // One 800 µs DPC: an individual event.
        c.feed(&ev(PERF_INFO, 68, 1_000_000 + 8_000, &dpc(1_000_000, netw)));
        // Thirty 100 µs DPCs inside one 10 ms bucket: total 3 ms > 2 ms.
        let b0 = 2_000_000;
        for i in 0..30 {
            let s = b0 + i * 3_000;
            c.feed(&ev(PERF_INFO, 68, s + 1_000, &dpc(s, netw)));
        }
        // A short ISR from an unknown address: unattributed, not an event.
        c.feed(&ev(PERF_INFO, 67, 3_000_100, &dpc(3_000_000, 0x1234)));
        let (events, unattributed) = c.finish();
        assert_eq!(unattributed, 1);
        let got: Vec<(&str, &str, u64)> = events
            .iter()
            .map(|e| (e.kind.as_str(), e.subject.as_str(), e.qpc))
            .collect();
        assert_eq!(
            got,
            [
                ("dpc", "Netwtw10.sys", 1_000_000),
                // The single DPC's own bucket is 0.8 ms: below 2 ms, no bucket event.
                ("dpc", "Netwtw10.sys", 2_000_000),
            ]
        );
        assert!((events[0].magnitude - 800.0).abs() < 1e-6);
        assert!((events[1].magnitude - 3000.0).abs() < 1e-6);
    }

    #[test]
    fn hard_faults_and_disk_io_are_attributed_to_processes() {
        let mut c = collector();
        // Five hard faults by thread 7 (stress.exe) inside 100 ms.
        for i in 0..6u64 {
            let mut d = vec![0u8; 32];
            d.extend(7u32.to_le_bytes());
            d.extend(4096u32.to_le_bytes());
            c.feed(&ev(PAGE_FAULT, 32, 5_000_000 + i * 10_000, &d));
        }
        // A 60 ms read by thread 7, and 25 MB written in one 250 ms window.
        let disk = |size: u32, resp: u64, tid: u32| {
            let mut d = vec![0u8; 8];
            d.extend(size.to_le_bytes());
            d.extend([0u8; 28]);
            d.extend(resp.to_le_bytes());
            d.extend(tid.to_le_bytes());
            assert_eq!(d.len(), 52);
            d
        };
        c.feed(&ev(DISK_IO, 10, 20_600_000, &disk(4096, 600_000, 7)));
        for i in 0..5u64 {
            c.feed(&ev(
                DISK_IO,
                11,
                31_000_000 + i * 1000,
                &disk(5 << 20, 100, 7),
            ));
        }
        // An unknown thread's slow I/O is blamed on "System".
        c.feed(&ev(DISK_IO, 10, 40_600_000, &disk(4096, 600_000, 99)));
        let (events, _) = c.finish();
        let got: Vec<(&str, &str)> = events
            .iter()
            .map(|e| (e.kind.as_str(), e.subject.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("hard_fault", "stress.exe"),
                ("disk_io", "stress.exe"),
                ("disk_io", "stress.exe"),
                ("disk_io", "System"),
            ]
        );
        assert_eq!(events[0].magnitude, 6.0);
        assert_eq!(events[1].qpc, 20_000_000); // start = completion − response time
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name(r"\SystemRoot\system32\drivers\a.sys"), "a.sys");
        assert_eq!(file_name("b.exe"), "b.exe");
    }
}
