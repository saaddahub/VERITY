//! The 4 Hz sampler behind Frame Story's sampled event kinds
//! (`cpu_burst`, `gpu_other`, `throttle`, `power_source`).
//!
//! Probed 2026-11-17: wildcard counters pick up processes started after the
//! query opened on the next collection, so no re-expansion is needed.

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::{process::qpc, wide};

const PDH_MORE_DATA: u32 = 0x8000_07D2;
const PROC_CPU: &str = r"\Process V2(*)\% Processor Time";
const GPU: &str = r"\GPU Engine(*)\Utilization Percentage";
const PERF_LIMIT: &str = r"\Processor Information(_Total)\% Performance Limit";

#[derive(Debug, Clone, PartialEq)]
pub struct StorySample {
    pub qpc: u64,
    /// (`name:pid` instance, percent of one core; can exceed 100).
    pub proc_cpu: Vec<(String, f32)>,
    /// (`GPU Engine` instance, utilization percent).
    pub gpu_engines: Vec<(String, f32)>,
    /// 100 means no limit; NaN when the counter is missing.
    pub perf_limit_pct: f32,
    /// None when Windows does not know.
    pub on_ac: Option<bool>,
}

pub struct StorySampler {
    query: PDH_HQUERY,
    proc_cpu: Option<PDH_HCOUNTER>,
    gpu: Option<PDH_HCOUNTER>,
    perf_limit: Option<PDH_HCOUNTER>,
}

// SAFETY: PDH handles are not thread-affine and `&mut self` serialises use.
unsafe impl Send for StorySampler {}

fn check(api: &'static str, status: u32) -> Result<(), OsError> {
    if status == 0 {
        Ok(())
    } else {
        Err(OsError { api, code: status })
    }
}

impl StorySampler {
    pub fn new() -> Result<Self, OsError> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: null data source means live data; `query` is an out-parameter.
        check("PdhOpenQueryW", unsafe {
            PdhOpenQueryW(PCWSTR::null(), 0, &mut query)
        })?;
        let mut s = Self {
            query,
            proc_cpu: None,
            gpu: None,
            perf_limit: None,
        };
        s.proc_cpu = s.add(PROC_CPU).ok();
        s.gpu = s.add(GPU).ok();
        s.perf_limit = s.add(PERF_LIMIT).ok();
        // SAFETY: `query` is open; rate counters need a first collection.
        let _ = unsafe { PdhCollectQueryData(s.query) };
        Ok(s)
    }

    fn add(&mut self, path: &str) -> Result<PDH_HCOUNTER, OsError> {
        let w = wide(path);
        let mut c = PDH_HCOUNTER::default();
        // SAFETY: `w` is NUL-terminated; `c` is an out-parameter.
        check("PdhAddEnglishCounterW", unsafe {
            PdhAddEnglishCounterW(self.query, PCWSTR(w.as_ptr()), 0, &mut c)
        })?;
        Ok(c)
    }

    fn value(&self, c: PDH_HCOUNTER) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: `c` belongs to this query; `v` is an out-parameter.
        let st = unsafe { PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE, None, &mut v) };
        // SAFETY: PDH_FMT_DOUBLE fills `doubleValue`.
        (st == 0 && v.CStatus == 0).then_some(unsafe { v.Anonymous.doubleValue })
    }

    fn array(&self, c: PDH_HCOUNTER) -> Vec<(String, f32)> {
        // Per-process CPU goes above 100% on multi-core machines; do not cap it.
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000); // PDH_FMT_NOCAP100
        let (mut bytes, mut count) = (0u32, 0u32);
        // SAFETY: a null buffer with size 0 asks for the required size.
        let st = unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut bytes, &mut count, None) };
        if st != PDH_MORE_DATA || bytes == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        // SAFETY: `buf` holds at least `bytes` bytes, 8-byte aligned.
        let st =
            unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut bytes, &mut count, Some(items)) };
        if st != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .filter_map(|i| {
                // SAFETY: PDH wrote `count` items into `buf`; names point inside it.
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

    pub fn sample(&mut self) -> Result<StorySample, OsError> {
        // SAFETY: `query` is open.
        check("PdhCollectQueryData", unsafe {
            PdhCollectQueryData(self.query)
        })?;
        let mut ps = SYSTEM_POWER_STATUS::default();
        // SAFETY: `ps` is an out-parameter.
        let read = unsafe { GetSystemPowerStatus(&mut ps) }.is_ok();
        let on_ac = match (read, ps.ACLineStatus) {
            (true, 0) => Some(false),
            (true, 1) => Some(true),
            _ => None,
        };
        Ok(StorySample {
            qpc: qpc(),
            proc_cpu: self.proc_cpu.map(|c| self.array(c)).unwrap_or_default(),
            gpu_engines: self.gpu.map(|c| self.array(c)).unwrap_or_default(),
            perf_limit_pct: self
                .perf_limit
                .and_then(|c| self.value(c))
                .unwrap_or(f64::NAN) as f32,
            on_ac,
        })
    }
}

impl Drop for StorySampler {
    fn drop(&mut self) {
        // SAFETY: `query` was opened in `new` and is closed once.
        let _ = unsafe { PdhCloseQuery(self.query) };
    }
}
