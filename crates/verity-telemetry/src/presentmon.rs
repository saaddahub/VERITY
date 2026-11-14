//! PresentMon console application as the frame source (docs/03-telemetry.md §1).
//!
//! Verified by running the pinned binary on Windows 11 build 26200:
//! - the flags in [`args`] exist under exactly these names;
//! - redirected stdout is UTF-16 LE with a byte-order mark;
//! - the `--v2_metrics` columns are those in [`REQUIRED`] (plus others), and a
//!   missing value is the text `NA`;
//! - capturing needs administrator rights.

use verity_core::metrics::{Frame, FLAG_DROPPED, FLAG_TEARING_ALLOWED};

pub const VERSION: &str = "2.6.0";
pub const FILE_NAME: &str = "PresentMon-2.6.0-x64.exe";
pub const SHA256: &str = "b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af";
/// Private ETW session name, so Verity never collides with another PresentMon.
pub const SESSION_NAME: &str = "VerityPM";

/// Command line that captures one process and writes CSV rows to stdout.
pub fn args(pid: u32) -> Vec<String> {
    [
        "--process_id",
        &pid.to_string(),
        "--output_stdout",
        "--qpc_time",
        "--v2_metrics",
        "--session_name",
        SESSION_NAME,
        "--stop_existing_session",
        "--terminate_on_proc_exit",
        "--no_console_stats",
    ]
    .map(String::from)
    .to_vec()
}

/// Command line that stops the running `VerityPM` session and exits, which
/// makes the capturing PresentMon finish cleanly.
pub fn stop_args() -> Vec<String> {
    [
        "--session_name",
        SESSION_NAME,
        "--terminate_existing_session",
    ]
    .map(String::from)
    .to_vec()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("PresentMon CSV has no `{0}` column; this build expects PresentMon {VERSION}")]
    MissingColumn(&'static str),
    #[error("PresentMon row has {got} fields, the header has {want}")]
    FieldCount { got: usize, want: usize },
    #[error("PresentMon column `{column}` has an unreadable value `{value}`")]
    BadValue { column: &'static str, value: String },
}

/// Turns PresentMon's stdout bytes into lines, whatever size the chunks arrive in.
#[derive(Default)]
pub struct LineDecoder {
    utf16: Option<bool>,
    bytes: Vec<u8>,
    text: String,
}

impl LineDecoder {
    /// Feeds bytes; returns the complete lines they finished.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.bytes.extend_from_slice(chunk);
        let utf16 = match self.utf16 {
            Some(u) => u,
            None => {
                if self.bytes.len() < 2 {
                    return Vec::new();
                }
                let bom = self.bytes.starts_with(&[0xFF, 0xFE]);
                if bom {
                    self.bytes.drain(..2);
                }
                self.utf16 = Some(bom);
                bom
            }
        };
        if utf16 {
            let whole = self.bytes.len() & !1;
            let units = self.bytes[..whole]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b));
            self.text
                .extend(char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')));
            self.bytes.drain(..whole);
        } else {
            // Only complete lines are decoded, so a split multi-byte character waits.
            let whole = self
                .bytes
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |i| i + 1);
            self.text
                .push_str(&String::from_utf8_lossy(&self.bytes[..whole]));
            self.bytes.drain(..whole);
        }
        let mut lines = Vec::new();
        while let Some(end) = self.text.find('\n') {
            let line: String = self.text.drain(..=end).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        lines
    }
}

const REQUIRED: [&str; 10] = [
    "Application",
    "ProcessID",
    "SwapChainAddress",
    "AllowsTearing",
    "PresentMode",
    "CPUStartQPC",
    "FrameTime",
    "CPUBusy",
    "GPUBusy",
    "DisplayedTime",
];
const DISPLAY_LATENCY: &str = "DisplayLatency";

/// Present modes in PresentMon's wording; a frame stores the index.
pub const PRESENT_MODES: [&str; 8] = [
    "Unknown",
    "Hardware: Legacy Flip",
    "Hardware: Legacy Copy to front buffer",
    "Hardware: Independent Flip",
    "Composed: Flip",
    "Composed: Copy with GPU GDI",
    "Composed: Copy with CPU GDI",
    "Hardware Composed: Independent Flip",
];

/// One CSV row: which process and swap chain, and the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub application: String,
    pub pid: u32,
    pub swap_chain: u64,
    pub frame: Frame,
}

/// Column positions, read from the header so column order may change.
#[derive(Debug, Clone)]
pub struct Layout {
    required: [usize; REQUIRED.len()],
    display_latency: Option<usize>,
    fields: usize,
}

impl Layout {
    /// Fails loudly when a column Verity depends on is missing.
    pub fn from_header(line: &str) -> Result<Self, ParseError> {
        let names: Vec<&str> = line.split(',').map(str::trim).collect();
        let find = |name: &str| names.iter().position(|n| *n == name);
        let mut required = [0; REQUIRED.len()];
        for (slot, name) in required.iter_mut().zip(REQUIRED) {
            *slot = find(name).ok_or(ParseError::MissingColumn(name))?;
        }
        Ok(Self {
            required,
            display_latency: find(DISPLAY_LATENCY),
            fields: names.len(),
        })
    }

    pub fn parse_row(&self, line: &str) -> Result<Row, ParseError> {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != self.fields {
            return Err(ParseError::FieldCount {
                got: f.len(),
                want: self.fields,
            });
        }
        let field = |i: usize| f[self.required[i]].trim();
        let bad = |i: usize| ParseError::BadValue {
            column: REQUIRED[i],
            value: field(i).to_string(),
        };
        // "NA" means not measured: NaN.
        let ms = |i: usize| match field(i) {
            "NA" => Ok(f32::NAN),
            v => v.parse::<f32>().map_err(|_| bad(i)),
        };

        let swap_chain =
            u64::from_str_radix(field(2).trim_start_matches("0x"), 16).map_err(|_| bad(2))?;
        let displayed_time_ms = ms(9)?;
        let mut flags = 0;
        if displayed_time_ms.is_nan() {
            flags |= FLAG_DROPPED;
        }
        if field(3) == "1" {
            flags |= FLAG_TEARING_ALLOWED;
        }
        Ok(Row {
            application: field(0).to_string(),
            pid: field(1).parse().map_err(|_| bad(1))?,
            swap_chain,
            frame: Frame {
                qpc: field(5).parse().map_err(|_| bad(5))?,
                frame_time_ms: ms(6)?,
                cpu_busy_ms: ms(7)?,
                gpu_busy_ms: ms(8)?,
                displayed_time_ms,
                display_latency_ms: match self.display_latency.map(|i| f[i].trim()) {
                    Some(v) if v != "NA" => v.parse().unwrap_or(f32::NAN),
                    _ => f32::NAN,
                },
                present_mode: PRESENT_MODES
                    .iter()
                    .position(|m| *m == field(4))
                    .unwrap_or(0) as u8,
                flags,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verity_core::metrics::{detect_hitches, summarize, Thresholds};

    /// A real capture of `framegen --fps 60 --duration 8 --hitch-at 2,4,6`.
    const CAPTURE: &[u8] = include_bytes!("../tests/fixtures/presentmon-2.6.0-framegen.csv");
    const TRUTH: &str = include_str!("../tests/fixtures/framegen-truth.csv");
    const FREQ: u64 = 10_000_000;

    fn rows(chunk_size: usize) -> Vec<Row> {
        let mut decoder = LineDecoder::default();
        let mut layout = None;
        let mut rows = Vec::new();
        for chunk in CAPTURE.chunks(chunk_size) {
            for line in decoder.push(chunk) {
                match &layout {
                    None => layout = Some(Layout::from_header(&line).unwrap()),
                    Some(l) => rows.push(l.parse_row(&line).unwrap()),
                }
            }
        }
        rows
    }

    #[test]
    fn real_capture_parses_the_same_in_any_chunking() {
        let whole = rows(usize::MAX);
        assert_eq!(whole.len(), 427);
        assert_eq!(whole[0].application, "framegen.exe");
        assert_eq!(whole[0].swap_chain, 0x19F0FCFF340);
        assert_eq!(whole[0].frame.qpc, 1580624186008);
        assert_eq!(
            PRESENT_MODES[whole[0].frame.present_mode as usize],
            "Hardware Composed: Independent Flip"
        );
        // Odd sizes split UTF-16 code units and lines mid-way.
        assert_eq!(rows(1), whole);
        assert_eq!(rows(4097), whole);
    }

    #[test]
    fn planted_hitches_are_found_and_nothing_else() {
        let frames: Vec<Frame> = rows(usize::MAX).into_iter().map(|r| r.frame).collect();
        let truth: Vec<u64> = TRUTH
            .lines()
            .filter_map(|l| l.strip_prefix("hitch,"))
            .map(|l| l.split(',').next().unwrap().parse().unwrap())
            .collect();
        assert_eq!(truth.len(), 3);

        let hitches = detect_hitches(&frames, &Thresholds::default());
        assert_eq!(
            hitches.len(),
            3,
            "exactly the planted hitches, no false ones"
        );
        let slack = 20 * FREQ / 1000; // 20 ms
        for (h, t) in hitches.iter().zip(&truth) {
            let end = h.qpc + (h.frame_time_ms as f64 / 1000.0 * FREQ as f64) as u64;
            assert!(
                h.qpc <= t + slack && *t <= end + slack,
                "planted hitch at {t} is not inside the long frame {}..{end}",
                h.qpc
            );
        }
    }

    #[test]
    fn capped_at_60_reads_as_60() {
        let frames: Vec<Frame> = rows(usize::MAX).into_iter().map(|r| r.frame).collect();
        let s = summarize(&frames, FREQ, &Thresholds::default());
        assert_eq!(s.hitch_groups.len(), 3);
        // Three 50 ms hitches in 7 s pull the average down by design; the
        // steady frames must sit within 1% of the 60 FPS cap.
        let mut times: Vec<f32> = frames.iter().map(|f| f.frame_time_ms).collect();
        times.sort_by(f32::total_cmp);
        let median_fps = 1000.0 / times[times.len() / 2];
        assert!((median_fps - 60.0).abs() < 0.6, "{median_fps}");
        assert!(s.avg_fps > 58.0 && s.avg_fps < 60.0, "{}", s.avg_fps);
    }

    #[test]
    fn utf8_input_also_works() {
        let mut d = LineDecoder::default();
        let mut lines = d.push(b"a,b\r\n1,");
        lines.extend(d.push(b"2\n"));
        assert_eq!(lines, ["a,b", "1,2"]);
    }

    #[test]
    fn layout_errors_are_specific() {
        assert_eq!(
            Layout::from_header("Application,ProcessID").unwrap_err(),
            ParseError::MissingColumn("SwapChainAddress")
        );
        let header = REQUIRED.join(",");
        let layout = Layout::from_header(&header).unwrap();
        assert!(matches!(
            layout.parse_row("a,1"),
            Err(ParseError::FieldCount { got: 2, want: 10 })
        ));
        let bad = "g.exe,1,0x10,0,Composed: Flip,notanumber,16.6,1,1,NA";
        assert!(matches!(
            layout.parse_row(bad),
            Err(ParseError::BadValue {
                column: "CPUStartQPC",
                ..
            })
        ));
        let dropped = layout
            .parse_row("g.exe,1,0x10,1,Composed: Flip,5,16.6,1,1,NA")
            .unwrap();
        assert_eq!(dropped.frame.flags, FLAG_DROPPED | FLAG_TEARING_ALLOWED);
        assert!(dropped.frame.display_latency_ms.is_nan());
    }

    #[test]
    fn command_line_targets_one_process() {
        let a = args(4242);
        assert_eq!(&a[..2], ["--process_id", "4242"]);
        assert!(a.iter().any(|x| x == "--terminate_on_proc_exit"));
    }
}
