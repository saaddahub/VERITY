use std::ffi::c_void;

use verity_core::{
    classify, vendor_name, Cpu, Display, Gpu, LogicalProcessor, OsError, PowerScheme, PowerState,
    SystemInfo, SystemInfoApi, WindowsVersion,
};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_MORE_DATA, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW,
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, PowerGetActiveScheme, PowerReadFriendlyName, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

use crate::{check, from_wide, hresult, wide};

const NT_CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU0: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

/// The real `SystemInfoApi`.
pub struct WinSystemInfo;

impl SystemInfoApi for WinSystemInfo {
    fn system_info(&self) -> Result<SystemInfo, OsError> {
        Ok(SystemInfo {
            windows: windows_version()?,
            cpu: cpu()?,
            gpus: gpus()?,
            displays: displays(),
            power: power()?,
            power_scheme: power_scheme()?,
        })
    }
}

fn reg_dword(subkey: &str, value: &str) -> Result<u32, OsError> {
    let (k, v) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: strings are NUL-terminated; `data`/`size` describe a valid 4-byte buffer.
    let e = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    check("RegGetValueW", e)?;
    Ok(data)
}

fn reg_string(subkey: &str, value: &str) -> Result<String, OsError> {
    let (k, v) = (wide(subkey), wide(value));
    let mut buf = vec![0u16; 128];
    loop {
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: strings are NUL-terminated; `size` is the byte length of `buf`.
        let e = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(k.as_ptr()),
                PCWSTR(v.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut c_void),
                Some(&mut size),
            )
        };
        if e == ERROR_MORE_DATA {
            buf.resize(size as usize / 2 + 1, 0);
            continue;
        }
        check("RegGetValueW", e)?;
        return Ok(from_wide(&buf));
    }
}

fn windows_version() -> Result<WindowsVersion, OsError> {
    let build = reg_string(NT_CURRENT_VERSION, "CurrentBuildNumber")?;
    Ok(WindowsVersion {
        major: reg_dword(NT_CURRENT_VERSION, "CurrentMajorVersionNumber")?,
        build: build.trim().parse().map_err(|_| OsError {
            api: "parse CurrentBuildNumber",
            code: 0,
        })?,
        ubr: reg_dword(NT_CURRENT_VERSION, "UBR").unwrap_or(0),
        display_version: reg_string(NT_CURRENT_VERSION, "DisplayVersion").ok(),
    })
}

fn cpu() -> Result<Cpu, OsError> {
    let raw = logical_processors()?;
    let classes: Vec<u8> = raw.iter().map(|&(_, _, c)| c).collect();
    let logical = raw
        .into_iter()
        .zip(classify(&classes))
        .map(
            |((group, number, efficiency_class), kind)| LogicalProcessor {
                group,
                number,
                efficiency_class,
                kind,
            },
        )
        .collect();
    Ok(Cpu {
        model: reg_string(CPU0, "ProcessorNameString")?.trim().to_string(),
        logical,
    })
}

/// (group, number within group, efficiency class) for every logical processor.
fn logical_processors() -> Result<Vec<(u16, u8, u8)>, OsError> {
    const API: &str = "GetLogicalProcessorInformationEx";
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 only asks for the required size.
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len) };
    if len == 0 {
        return Err(OsError { api: API, code: 0 });
    }
    // u64 storage keeps every record 8-byte aligned; record sizes are multiples of 8.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    unsafe {
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            Some(buf.as_mut_ptr().cast()),
            &mut len,
        )
    }
    .map_err(hresult(API))?;

    let base = buf.as_ptr() as *const u8;
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < len as usize {
        // SAFETY: the OS wrote `len` bytes of consecutive, aligned records; each
        // record's `Size` is its stride and `offset` stays inside the buffer.
        let rec = unsafe { &*(base.add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
        if rec.Relationship == RelationProcessorCore {
            // SAFETY: for RelationProcessorCore the union holds `Processor`, whose
            // GroupMask array has GroupCount entries inside this record.
            let (class, masks) = unsafe {
                let p = &rec.Anonymous.Processor;
                (
                    p.EfficiencyClass,
                    std::slice::from_raw_parts(p.GroupMask.as_ptr(), p.GroupCount as usize),
                )
            };
            for m in masks {
                for bit in 0..usize::BITS {
                    if m.Mask & (1usize << bit) != 0 {
                        out.push((m.Group, bit as u8, class));
                    }
                }
            }
        }
        if rec.Size == 0 {
            break;
        }
        offset += rec.Size as usize;
    }
    out.sort_unstable();
    Ok(out)
}

fn gpus() -> Result<Vec<Gpu>, OsError> {
    // SAFETY: factory creation has no preconditions.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(hresult("CreateDXGIFactory1"))?;
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for i in 0.. {
        // SAFETY: `factory` is valid; DXGI_ERROR_NOT_FOUND ends enumeration.
        let adapter = match unsafe { factory.EnumAdapters1(i) } {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(hresult("IDXGIFactory1::EnumAdapters1")(e)),
        };
        // SAFETY: `adapter` is a valid adapter returned above.
        let d = unsafe { adapter.GetDesc1() }.map_err(hresult("IDXGIAdapter1::GetDesc1"))?;
        // Virtual display adapters (Parsec and the like) show up as a second
        // entry with the hardware IDs of the GPU that renders for them.
        // shortcut: two identical physical GPUs collapse into one; key on the
        // PnP instance if multi-GPU desktops ever matter.
        let chip = (d.VendorId, d.DeviceId, d.SubSysId, d.Revision);
        let luid = (d.AdapterLuid.HighPart as u64) << 32 | d.AdapterLuid.LowPart as u64;
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        if let Some(i) = seen.iter().position(|c| *c == chip) {
            let known: &mut Gpu = &mut out[i];
            known.luids.push(luid);
            continue;
        }
        seen.push(chip);
        out.push(Gpu {
            name: from_wide(&d.Description),
            vendor_id: d.VendorId,
            vendor: vendor_name(d.VendorId).to_string(),
            dedicated_vram_bytes: d.DedicatedVideoMemory as u64,
            luids: vec![luid],
        });
    }
    Ok(out)
}

fn devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    }
}

fn display_device() -> DISPLAY_DEVICEW {
    DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    }
}

/// Desktop-attached displays as seen from the calling session. From session 0
/// (the service) this may be empty or wrong; the CLI reads its own.
pub fn displays() -> Vec<Display> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut dd = display_device();
        // SAFETY: `dd.cb` is set; a null device name enumerates display outputs.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
            break;
        }
        if (dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP).0 == 0 {
            continue;
        }
        let device = from_wide(&dd.DeviceName);
        let name = wide(&device);
        let mut cur = devmode();
        // SAFETY: `name` is NUL-terminated; `cur.dmSize` is set.
        if !unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut cur) }
            .as_bool()
        {
            continue;
        }
        let mut max_hz = cur.dmDisplayFrequency;
        for mode in 0.. {
            let mut dm = devmode();
            // SAFETY: as above; a false return ends the mode list.
            if !unsafe {
                EnumDisplaySettingsW(
                    PCWSTR(name.as_ptr()),
                    ENUM_DISPLAY_SETTINGS_MODE(mode),
                    &mut dm,
                )
            }
            .as_bool()
            {
                break;
            }
            if dm.dmPelsWidth == cur.dmPelsWidth && dm.dmPelsHeight == cur.dmPelsHeight {
                max_hz = max_hz.max(dm.dmDisplayFrequency);
            }
        }
        out.push(Display {
            device,
            width: cur.dmPelsWidth,
            height: cur.dmPelsHeight,
            current_hz: cur.dmDisplayFrequency,
            max_hz,
        });
    }
    out
}

fn power() -> Result<PowerState, OsError> {
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: `s` is a valid out-parameter.
    unsafe { GetSystemPowerStatus(&mut s) }.map_err(hresult("GetSystemPowerStatus"))?;
    Ok(PowerState::from_raw(
        s.ACLineStatus,
        s.BatteryFlag,
        s.BatteryLifePercent,
    ))
}

fn power_scheme() -> Result<PowerScheme, OsError> {
    let mut active: *mut GUID = std::ptr::null_mut();
    // SAFETY: on success `active` points to a LocalAlloc'd GUID that we own.
    check("PowerGetActiveScheme", unsafe {
        PowerGetActiveScheme(None, &mut active)
    })?;
    // SAFETY: `active` is valid (checked above); copy it, then free it once.
    let guid = unsafe {
        let g = *active;
        let _ = LocalFree(Some(HLOCAL(active.cast())));
        g
    };

    let mut size = 0u32;
    // SAFETY: a null buffer asks for the required size in bytes.
    let _ = unsafe { PowerReadFriendlyName(None, Some(&guid), None, None, None, &mut size) };
    let name = (size > 0)
        .then(|| {
            let mut buf = vec![0u16; size as usize / 2 + 1];
            let mut bytes = (buf.len() * 2) as u32;
            // SAFETY: `bytes` is the byte length of `buf`.
            let e = unsafe {
                PowerReadFriendlyName(
                    None,
                    Some(&guid),
                    None,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    &mut bytes,
                )
            };
            (e == ERROR_SUCCESS).then(|| from_wide(&buf))
        })
        .flatten();

    Ok(PowerScheme {
        guid: format!("{guid:?}").to_lowercase(),
        name,
    })
}
