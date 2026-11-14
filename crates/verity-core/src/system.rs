use serde::{Deserialize, Serialize};

use crate::OsError;

/// Reads a hardware and OS snapshot.
pub trait SystemInfoApi: Send + Sync {
    fn system_info(&self) -> Result<SystemInfo, OsError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub windows: WindowsVersion,
    pub cpu: Cpu,
    pub gpus: Vec<Gpu>,
    pub displays: Vec<Display>,
    pub power: PowerState,
    pub power_scheme: PowerScheme,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowsVersion {
    pub major: u32,
    pub build: u32,
    pub ubr: u32,
    /// "24H2" and the like; absent on older builds.
    pub display_version: Option<String>,
}

impl WindowsVersion {
    /// The registry's ProductName says "Windows 10" on Windows 11, so go by build number.
    pub fn product_name(&self) -> &'static str {
        if self.build >= 22000 {
            "Windows 11"
        } else {
            "Windows 10"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    pub model: String,
    pub logical: Vec<LogicalProcessor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogicalProcessor {
    pub group: u16,
    pub number: u8,
    /// Raw Windows efficiency class: higher means more performant.
    pub efficiency_class: u8,
    pub kind: CoreKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreKind {
    Performance,
    Efficiency,
    /// Not a hybrid CPU: every core has the same class.
    Uniform,
}

/// Classes are relative: the highest class is performance, anything lower is efficiency.
pub fn classify(classes: &[u8]) -> Vec<CoreKind> {
    let (Some(&min), Some(&max)) = (classes.iter().min(), classes.iter().max()) else {
        return Vec::new();
    };
    classes
        .iter()
        .map(|&c| match c {
            _ if min == max => CoreKind::Uniform,
            c if c == max => CoreKind::Performance,
            _ => CoreKind::Efficiency,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    pub vendor_id: u32,
    pub vendor: String,
    pub dedicated_vram_bytes: u64,
    /// Every adapter LUID that is this chip (virtual display adapters add extras).
    #[serde(default)]
    pub luids: Vec<u64>,
}

pub fn vendor_name(vendor_id: u32) -> &'static str {
    match vendor_id {
        0x10DE => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x1414 => "Microsoft",
        0x5143 => "Qualcomm",
        _ => "Unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub current_hz: u32,
    /// Highest refresh offered at the current resolution.
    pub max_hz: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerState {
    /// None when Windows reports the AC state as unknown.
    pub ac_online: Option<bool>,
    pub battery_present: bool,
    pub charging: bool,
    pub battery_percent: Option<u8>,
}

impl PowerState {
    /// Decodes the raw `SYSTEM_POWER_STATUS` fields.
    pub fn from_raw(ac_line: u8, battery_flag: u8, life_percent: u8) -> Self {
        const NO_BATTERY: u8 = 128;
        const CHARGING: u8 = 8;
        const UNKNOWN: u8 = 255;
        let known = battery_flag != UNKNOWN;
        Self {
            ac_online: match ac_line {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            battery_present: known && battery_flag & NO_BATTERY == 0,
            charging: known && battery_flag & CHARGING != 0,
            battery_percent: (life_percent <= 100).then_some(life_percent),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerScheme {
    pub guid: String,
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_cpu_splits_by_highest_class() {
        use CoreKind::*;
        assert_eq!(
            classify(&[1, 1, 0, 0]),
            [Performance, Performance, Efficiency, Efficiency]
        );
        // Three tiers (low-power E-cores): only the top tier is performance.
        assert_eq!(classify(&[2, 1, 0]), [Performance, Efficiency, Efficiency]);
    }

    #[test]
    fn uniform_and_empty_cpus() {
        assert_eq!(classify(&[0, 0]), [CoreKind::Uniform, CoreKind::Uniform]);
        assert!(classify(&[]).is_empty());
    }

    #[test]
    fn power_status_decoding() {
        let laptop = PowerState::from_raw(1, 8, 87);
        assert_eq!(laptop.ac_online, Some(true));
        assert!(laptop.battery_present && laptop.charging);
        assert_eq!(laptop.battery_percent, Some(87));

        let desktop = PowerState::from_raw(1, 128, 255);
        assert!(!desktop.battery_present && !desktop.charging);
        assert_eq!(desktop.battery_percent, None);

        let unknown = PowerState::from_raw(255, 255, 255);
        assert_eq!(unknown.ac_online, None);
        assert!(!unknown.battery_present);
    }

    #[test]
    fn windows_11_detected_by_build() {
        let v = |build| WindowsVersion {
            major: 10,
            build,
            ubr: 0,
            display_version: None,
        };
        assert_eq!(v(19045).product_name(), "Windows 10");
        assert_eq!(v(26100).product_name(), "Windows 11");
    }

    #[test]
    fn vendors() {
        assert_eq!(vendor_name(0x10DE), "NVIDIA");
        assert_eq!(vendor_name(0xFFFF), "Unknown");
    }
}
