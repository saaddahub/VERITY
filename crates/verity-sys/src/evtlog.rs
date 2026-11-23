//! Event-log subscriptions (`EvtSubscribe`) for Defender scans, Windows
//! Update and scheduled tasks (docs/03-telemetry.md §5). Each delivered event
//! is rendered as XML and handed to the callback; parsing is the caller's.

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::System::EventLog::{EvtChannelConfigEnabled, EVT_VARIANT};
use windows::Win32::System::EventLog::{
    EvtClose, EvtRender, EvtRenderEventXml, EvtSubscribe, EvtSubscribeActionDeliver,
    EvtSubscribeToFutureEvents, EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION,
};
use windows::Win32::System::EventLog::{EvtGetChannelConfigProperty, EvtOpenChannelConfig};

use crate::{hresult, wide};

type Cb = Box<dyn FnMut(String) + Send>;

pub struct Subscription {
    handle: EVT_HANDLE,
    ctx: *mut Cb,
}

// SAFETY: the handle may be closed from any thread; `ctx` is only touched by
// the event-log callback thread until `drop`, after EvtClose has returned.
unsafe impl Send for Subscription {}

unsafe extern "system" fn on_event(
    action: EVT_SUBSCRIBE_NOTIFY_ACTION,
    ctx: *const core::ffi::c_void,
    event: EVT_HANDLE,
) -> u32 {
    if action != EvtSubscribeActionDeliver || ctx.is_null() {
        return 0;
    }
    let (mut used, mut props) = (0u32, 0u32);
    // SAFETY: size query with no buffer; fails with ERROR_INSUFFICIENT_BUFFER.
    let _ = unsafe {
        EvtRender(
            None,
            event,
            EvtRenderEventXml.0,
            0,
            None,
            &mut used,
            &mut props,
        )
    };
    if used == 0 || used > 1 << 20 {
        return 0;
    }
    let mut buf = vec![0u16; (used as usize).div_ceil(2)];
    // SAFETY: `buf` holds `used` bytes; `event` is valid during the callback.
    let ok = unsafe {
        EvtRender(
            None,
            event,
            EvtRenderEventXml.0,
            used,
            Some(buf.as_mut_ptr() as *mut _),
            &mut used,
            &mut props,
        )
    }
    .is_ok();
    if ok {
        let xml = crate::from_wide(&buf);
        // SAFETY: `ctx` is the boxed callback owned by the Subscription, alive
        // until EvtClose returns, and called from one delivery thread at a time.
        let cb = unsafe { &mut *(ctx as *mut Cb) };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(xml)));
    }
    0
}

/// Subscribes to new events on `channel` (a fixed channel name, never client input).
pub fn subscribe(channel: &str, cb: Cb) -> Result<Subscription, OsError> {
    let ch = wide(channel);
    let query = wide("*");
    let ctx = Box::into_raw(Box::new(cb));
    // SAFETY: strings are NUL-terminated; `ctx` stays valid until drop.
    let handle = unsafe {
        EvtSubscribe(
            None,
            None,
            PCWSTR(ch.as_ptr()),
            PCWSTR(query.as_ptr()),
            None,
            Some(ctx as *const _),
            Some(on_event),
            EvtSubscribeToFutureEvents.0,
        )
    };
    match handle {
        Ok(handle) => Ok(Subscription { handle, ctx }),
        Err(e) => {
            // SAFETY: `ctx` came from Box::into_raw and was never used.
            drop(unsafe { Box::from_raw(ctx) });
            Err(hresult("EvtSubscribe")(e))
        }
    }
}

/// Whether a channel is enabled; None if it cannot be read.
pub fn channel_enabled(channel: &str) -> Option<bool> {
    let ch = wide(channel);
    // SAFETY: `ch` is NUL-terminated; the handle is closed below.
    let cfg = unsafe { EvtOpenChannelConfig(None, PCWSTR(ch.as_ptr()), 0) }.ok()?;
    let mut v = EVT_VARIANT::default();
    let mut used = 0u32;
    // SAFETY: `v` is a full EVT_VARIANT; a boolean property fits in it.
    let r = unsafe {
        EvtGetChannelConfigProperty(
            cfg,
            EvtChannelConfigEnabled,
            0,
            std::mem::size_of::<EVT_VARIANT>() as u32,
            Some(&mut v),
            &mut used,
        )
    };
    // SAFETY: `cfg` came from EvtOpenChannelConfig and is closed once.
    let _ = unsafe { EvtClose(cfg) };
    r.ok()?;
    // SAFETY: the Enabled property is an EvtVarTypeBoolean (a BOOL).
    Some(unsafe { v.Anonymous.BooleanVal }.as_bool())
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // SAFETY: `handle` came from EvtSubscribe; EvtClose waits for callbacks.
        let _ = unsafe { EvtClose(self.handle) };
        // SAFETY: no callback can run after EvtClose; `ctx` is freed once.
        drop(unsafe { Box::from_raw(self.ctx) });
    }
}
