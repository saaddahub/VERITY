use verity_core::telemetry::{CounterApi, SystemSample};
use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};

use crate::{process::qpc, wide};

const PDH_MORE_DATA: u32 = 0x8000_07D2;

fn check(api: &'static str, status: u32) -> Result<(), OsError> {
    if status == 0 {
        Ok(())
    } else {
        Err(OsError { api, code: status })
    }
}

/// The real `CounterApi`: one PDH query, counters added by English name so
/// non-English Windows works.
pub struct PdhCounters {
    query: PDH_HQUERY,
    cpu: PDH_HCOUNTER,
    mem: PDH_HCOUNTER,
    /// None when the machine has no GPU Engine counters.
    gpu: Option<PDH_HCOUNTER>,
}

// SAFETY: PDH query handles are not tied to a thread, and `&mut self` on every
// method means only one thread uses the handles at a time.
unsafe impl Send for PdhCounters {}

impl PdhCounters {
    pub fn new() -> Result<Self, OsError> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: a null data source means live data; `query` is a valid out-parameter.
        check("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        let mut this = Self {
            query,
            cpu: PDH_HCOUNTER::default(),
            mem: PDH_HCOUNTER::default(),
            gpu: None,
        };
        this.cpu = this.add(r"\Processor Information(_Total)\% Processor Utility")?;
        this.mem = this.add(r"\Memory\Available MBytes")?;
        // shortcut: instances are those present when the query is opened; re-add
        // the wildcard every 5 s (docs/03-telemetry.md §2) once Session Mode
        // starts capturing before the game does.
        this.gpu = this.add(r"\GPU Engine(*)\Utilization Percentage").ok();
        // Rate counters need two collections; this is the first.
        // SAFETY: `query` is a valid open query.
        let _ = unsafe { PdhCollectQueryData(this.query) };
        Ok(this)
    }

    fn add(&mut self, path: &str) -> Result<PDH_HCOUNTER, OsError> {
        let w = wide(path);
        let mut counter = PDH_HCOUNTER::default();
        // SAFETY: `w` is NUL-terminated; `counter` is a valid out-parameter.
        check("PdhAddEnglishCounterW", unsafe {
            PdhAddEnglishCounterW(self.query, PCWSTR(w.as_ptr()), 0, &mut counter)
        })?;
        Ok(counter)
    }

    fn value(&self, counter: PDH_HCOUNTER) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: `counter` belongs to this query; `v` is a valid out-parameter.
        let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut v) };
        // SAFETY: PDH_FMT_DOUBLE was requested, so the union holds `doubleValue`;
        // reading it is harmless even when the status says the value is unusable.
        (status == 0 && v.CStatus == 0).then_some(unsafe { v.Anonymous.doubleValue })
    }

    fn array(&self, counter: PDH_HCOUNTER) -> Vec<(String, f32)> {
        let (mut bytes, mut count) = (0u32, 0u32);
        // SAFETY: a null buffer with size 0 asks for the required size.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None)
        };
        if status != PDH_MORE_DATA || bytes == 0 {
            return Vec::new();
        }
        // u64 storage keeps the items 8-byte aligned.
        let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        // SAFETY: `buf` holds at least `bytes` bytes.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                Some(items),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .filter_map(|i| {
                // SAFETY: PDH wrote `count` items at the start of `buf`; each name
                // points to a NUL-terminated string inside the same buffer.
                unsafe {
                    let item = &*items.add(i);
                    (item.FmtValue.CStatus == 0).then(|| {
                        (
                            item.szName.to_string().unwrap_or_default(),
                            item.FmtValue.Anonymous.doubleValue as f32,
                        )
                    })
                }
            })
            .collect()
    }
}

impl CounterApi for PdhCounters {
    fn sample(&mut self) -> Result<SystemSample, OsError> {
        // SAFETY: `query` is a valid open query.
        check("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(self.query)
        })?;
        Ok(SystemSample {
            qpc: qpc(),
            cpu_total_pct: self.value(self.cpu).unwrap_or(f64::NAN) as f32,
            mem_available_mb: self.value(self.mem).unwrap_or(f64::NAN) as f32,
            gpu_engines: self.gpu.map(|c| self.array(c)).unwrap_or_default(),
        })
    }
}

impl Drop for PdhCounters {
    fn drop(&mut self) {
        // SAFETY: `query` was opened by PdhOpenQueryW and is closed once.
        let _ = unsafe { PdhCloseQuery(self.query) };
    }
}
