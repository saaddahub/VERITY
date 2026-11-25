//! System Restore points (docs/09-safety.md §1.4): one before Verity's first
//! persistent change. Needs administrator rights; the service calls it.

use verity_core::OsError;
use windows::Win32::Foundation::ERROR_SERVICE_DISABLED;
use windows::Win32::System::Restore::{
    SRSetRestorePointW, BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS, RESTOREPOINTINFOW,
    STATEMGRSTATUS,
};

use crate::wmi::Wmi;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePoint {
    Created,
    /// Windows makes at most one restore point a day and skipped this one.
    Skipped,
    /// System Restore is turned off on this PC.
    Disabled,
}

/// The newest restore point's sequence number, read through WMI.
fn newest() -> Result<Option<u64>, OsError> {
    let wmi = Wmi::connect(r"ROOT\DEFAULT")?;
    let rows = wmi.query(
        "SELECT SequenceNumber FROM SystemRestore",
        &["SequenceNumber"],
    )?;
    Ok(rows.iter().filter_map(|r| r.first()?.as_u64()).max())
}

/// Asks Windows for a restore point named `description` and reports whether
/// one was actually made.
pub fn create(description: &str) -> Result<RestorePoint, OsError> {
    let before = newest()?;
    // The struct is packed: build the description apart, then copy it in whole.
    let mut text = [0u16; 256];
    for (dst, src) in text.iter_mut().zip(description.encode_utf16().take(255)) {
        *dst = src;
    }
    let info = RESTOREPOINTINFOW {
        dwEventType: BEGIN_SYSTEM_CHANGE,
        dwRestorePtType: MODIFY_SETTINGS,
        llSequenceNumber: 0,
        szDescription: text,
    };
    let mut status = STATEMGRSTATUS::default();
    // SAFETY: `info` and `status` are valid, correctly sized structures.
    if !unsafe { SRSetRestorePointW(&info, &mut status) }.as_bool() {
        let code = { status.nStatus };
        if code == ERROR_SERVICE_DISABLED {
            return Ok(RestorePoint::Disabled);
        }
        return Err(OsError {
            api: "SRSetRestorePointW",
            code: code.0,
        });
    }
    let end = RESTOREPOINTINFOW {
        dwEventType: END_SYSTEM_CHANGE,
        llSequenceNumber: status.llSequenceNumber,
        ..Default::default()
    };
    // SAFETY: as above.
    if !unsafe { SRSetRestorePointW(&end, &mut status) }.as_bool() {
        let code = { status.nStatus };
        return Err(OsError {
            api: "SRSetRestorePointW",
            code: code.0,
        });
    }
    Ok(if newest()? > before {
        RestorePoint::Created
    } else {
        RestorePoint::Skipped
    })
}
