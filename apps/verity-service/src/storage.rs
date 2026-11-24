//! Drive health and Optimize Drives for the Storage screen (docs/14-storage.md).
//! Needs admin rights, so it runs here; it reads disk counters only, never
//! file names.

use serde::Serialize;
use verity_sys::wmi::{Row, Value, Wmi};

const NS: &str = r"ROOT\Microsoft\Windows\Storage";
/// `__PATH` last, so the positions `disks_from` reads stay fixed.
const DISK_PROPS: [&str; 7] = [
    "DeviceId",
    "FriendlyName",
    "MediaType",
    "HealthStatus",
    "BusType",
    "Size",
    "__PATH",
];
const RELIABILITY_PROPS: [&str; 7] = [
    "DeviceId",
    "Temperature",
    "TemperatureMax",
    "Wear",
    "ReadErrorsTotal",
    "WriteErrorsTotal",
    "PowerOnHours",
];
/// `__RELPATH`: ExecMethod wants the namespace-relative path; the full
/// `__PATH` (with machine and namespace) fails with WBEM_E_INVALID_PARAMETER.
const VOLUME_PROPS: [&str; 5] = [
    "__RELPATH",
    "DriveLetter",
    "FileSystem",
    "Size",
    "SizeRemaining",
];
/// `BusType` of a file-backed virtual disk (a mounted VHD).
const BUS_VIRTUAL: u64 = 15;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Disk {
    pub model: String,
    /// "SSD", "HDD", "SCM" or "Unknown".
    pub media: &'static str,
    /// "Healthy", "Warning", "Unhealthy" or "Unknown".
    pub health: &'static str,
    pub size: u64,
    /// Percent of rated life used; some drives always report 0.
    pub wear: Option<u64>,
    pub temperature: Option<u64>,
    pub temperature_max: Option<u64>,
    pub read_errors: Option<u64>,
    pub write_errors: Option<u64>,
    pub power_on_hours: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Volume {
    pub letter: char,
    pub file_system: String,
    pub size: u64,
    pub free: u64,
    #[serde(skip)]
    path: String,
}

fn get(r: &Row, i: usize) -> Option<u64> {
    r.get(i).and_then(Value::as_u64)
}

fn text(r: &Row, i: usize) -> String {
    r.get(i)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Joins physical disks with their reliability counters by `DeviceId`.
/// Virtual disks are left out.
pub fn disks_from(disks: &[Row], reliability: &[Row]) -> Vec<Disk> {
    disks
        .iter()
        .filter(|d| get(d, 4) != Some(BUS_VIRTUAL))
        .map(|d| {
            let id = text(d, 0);
            let rel = reliability.iter().find(|r| text(r, 0) == id);
            let counter = |i: usize| rel.and_then(|r| get(r, i));
            Disk {
                model: text(d, 1),
                media: match get(d, 2) {
                    Some(3) => "HDD",
                    Some(4) => "SSD",
                    Some(5) => "SCM",
                    _ => "Unknown",
                },
                health: match get(d, 3) {
                    Some(0) => "Healthy",
                    Some(1) => "Warning",
                    Some(2) => "Unhealthy",
                    _ => "Unknown",
                },
                size: get(d, 5).unwrap_or(0),
                temperature: counter(1).filter(|&t| t > 0),
                temperature_max: counter(2).filter(|&t| t > 0),
                wear: counter(3),
                read_errors: counter(4),
                write_errors: counter(5),
                power_on_hours: counter(6),
            }
        })
        .collect()
}

/// Fixed volumes that have a drive letter.
pub fn volumes_from(rows: &[Row]) -> Vec<Volume> {
    rows.iter()
        .filter_map(|r| {
            let letter = char::from_u32(get(r, 1)? as u32).filter(char::is_ascii_alphabetic)?;
            Some(Volume {
                letter: letter.to_ascii_uppercase(),
                file_system: text(r, 2),
                size: get(r, 3).unwrap_or(0),
                free: get(r, 4).unwrap_or(0),
                path: text(r, 0),
            })
        })
        .collect()
}

fn select(props: &[&str], class: &str, filter: &str) -> String {
    format!("SELECT {} FROM {class}{filter}", props.join(", "))
}

fn volumes(w: &Wmi) -> Result<Vec<Volume>, String> {
    let rows = w
        .query(
            &select(&VOLUME_PROPS, "MSFT_Volume", " WHERE DriveType = 3"),
            &VOLUME_PROPS,
        )
        .map_err(|e| e.to_string())?;
    Ok(volumes_from(&rows))
}

/// Disks with health and wear, and the fixed volumes. Blocking.
pub fn health() -> Result<(Vec<Disk>, Vec<Volume>), String> {
    let w = Wmi::connect(NS).map_err(|e| e.to_string())?;
    let disks = w
        .query(&select(&DISK_PROPS, "MSFT_PhysicalDisk", ""), &DISK_PROPS)
        .map_err(|e| e.to_string())?;
    // Reliability counters exist only through each disk's association: a plain
    // SELECT returns no rows even as SYSTEM (probed, docs/notes/decisions.md).
    // Missing counters are shown as "not reported", not as an error.
    let mut reliability = Vec::new();
    for d in &disks {
        let Some(path) = d.get(6).and_then(Value::as_str) else {
            continue;
        };
        let wql =
            format!("ASSOCIATORS OF {{{path}}} WHERE ResultClass = MSFT_StorageReliabilityCounter");
        match w.query(&wql, &RELIABILITY_PROPS) {
            Ok(rows) => reliability.extend(rows),
            Err(e) => tracing::warn!("reliability counters unavailable: {e}"),
        }
    }
    Ok((disks_from(&disks, &reliability), volumes(&w)?))
}

/// Runs Optimize Drives (TRIM on an SSD, defragmentation on an HDD) on the
/// volume with this letter, which must be one Windows itself lists. Blocking;
/// can take minutes. Returns the method's result code (0 is success).
pub fn optimize(letter: char) -> Result<u64, String> {
    let w = Wmi::connect(NS).map_err(|e| e.to_string())?;
    let v = volumes(&w)?
        .into_iter()
        .find(|v| v.letter == letter.to_ascii_uppercase())
        .ok_or_else(|| format!("{letter}: is not a fixed drive on this PC"))?;
    let arg = optimize_arg(media_of(&w, &v.path))?;
    w.call("MSFT_Volume", &v.path, "Optimize", &[(arg, true)])
        .map_err(|e| e.to_string())
}

/// The `MediaType` of the physical disk under a volume: volume, then its
/// partition's disk number, then the physical disk with that `DeviceId`
/// (probed on the dev laptop: C: is on disk 0, an SSD).
fn media_of(w: &Wmi, volume_path: &str) -> Option<u64> {
    let parts = w
        .query(
            &format!("ASSOCIATORS OF {{{volume_path}}} WHERE ResultClass = MSFT_Partition"),
            &["DiskNumber"],
        )
        .ok()?;
    let disk = parts.first()?.first()?.as_u64()?;
    let rows = w
        .query(
            &format!("SELECT MediaType FROM MSFT_PhysicalDisk WHERE DeviceId = '{disk}'"),
            &["MediaType"],
        )
        .ok()?;
    rows.first()?.first()?.as_u64()
}

/// What Optimize must do: the method does nothing without an operation, unlike
/// the Optimize-Volume command, which picks one by drive type.
fn optimize_arg(media: Option<u64>) -> Result<&'static str, String> {
    match media {
        Some(3) => Ok("Defrag"),
        Some(4) | Some(5) => Ok("ReTrim"),
        _ => Err(
            "Verity cannot tell what kind of drive this is. Use Optimize Drives in Windows instead."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn disks_join_counters_and_drop_virtual_disks() {
        // Shapes as probed on the dev laptop (docs/notes/decisions.md).
        let disks = vec![
            vec![
                t("1"),
                t("Msft Virtual Disk"),
                Value::Int(4),
                Value::Int(0),
                Value::Int(15),
                t("33554432"),
            ],
            vec![
                t("0"),
                t("NVMe Micron_2400 "),
                Value::Int(4),
                Value::Int(0),
                Value::Int(17),
                t("512110190592"),
            ],
        ];
        let rel = vec![vec![
            t("0"),
            Value::Int(41),
            Value::Int(84),
            Value::Int(3),
            t("0"),
            t("0"),
            Value::Int(5321),
        ]];
        let d = disks_from(&disks, &rel);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].model, "NVMe Micron_2400");
        assert_eq!((d[0].media, d[0].health), ("SSD", "Healthy"));
        assert_eq!(d[0].size, 512_110_190_592);
        assert_eq!(
            (d[0].wear, d[0].temperature, d[0].power_on_hours),
            (Some(3), Some(41), Some(5321))
        );
        // Without counters (not elevated, or not supported): reported as None.
        let bare = disks_from(&disks[1..], &[]);
        assert_eq!((bare[0].wear, bare[0].temperature), (None, None));
    }

    #[test]
    fn optimize_picks_the_operation_by_drive_type() {
        assert_eq!(optimize_arg(Some(4)), Ok("ReTrim"));
        assert_eq!(optimize_arg(Some(5)), Ok("ReTrim"));
        assert_eq!(optimize_arg(Some(3)), Ok("Defrag"));
        assert!(optimize_arg(Some(0)).is_err());
        assert!(optimize_arg(None).is_err());
    }

    #[test]
    fn only_lettered_volumes() {
        let rows = vec![
            vec![t("p0"), Value::Int(0), t("NTFS"), t("100"), t("50")],
            vec![
                t("pC"),
                Value::Int(67),
                t("NTFS"),
                t("484569321472"),
                t("109058310144"),
            ],
        ];
        let v = volumes_from(&rows);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].letter, v[0].free), ('C', 109_058_310_144));
        assert_eq!(v[0].path, "pC");
    }
}
