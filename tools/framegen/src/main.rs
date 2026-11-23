//! framegen: a synthetic "game" for Verity's tests. Presents Direct3D 11 frames
//! at a known rate with planted hitches and writes a ground-truth log, so the
//! capture pipeline can be checked against known answers.
//!
//! Test tooling: its Win32 calls live here, not in `verity-sys` (see
//! docs/notes/decisions.md).
//!
//! shortcut: `--gpu-load` from docs/11-roadmap.md is not implemented; add it
//! when M2 (rule D050) needs it.

use std::io::Write;
use std::process::ExitCode;
use std::str::FromStr;

use windows::core::w;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDeviceAndSwapChain, ID3D11Texture2D, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CW_USEDEFAULT, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_DESTROY,
    WM_QUIT, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const USAGE: &str = "usage: framegen [--fps <n> | --uncapped] [--cost-ms <x>]
                [--hitch-at <s1,s2,...> --hitch-ms <x>] [--hitch-on-event <name>]
                [--duration <s>] [--log <path>]
defaults: --fps 60, --duration 10, --log framegen-truth.csv";

#[derive(Debug, PartialEq)]
struct Config {
    /// None = uncapped.
    fps: Option<f64>,
    cost_ms: f64,
    hitch_at_s: Vec<f64>,
    hitch_ms: f64,
    /// Named event (Local\ namespace): each signal plants one hitch.
    hitch_event: Option<String>,
    duration_s: f64,
    log: String,
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut c = Config {
        fps: Some(60.0),
        cost_ms: 0.0,
        hitch_at_s: Vec::new(),
        hitch_ms: 50.0,
        hitch_event: None,
        duration_s: 10.0,
        log: "framegen-truth.csv".into(),
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag == "--uncapped" {
            c.fps = None;
            continue;
        }
        let value = it
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let num = || f64::from_str(value).map_err(|_| format!("bad number for {flag}: {value}"));
        match flag.as_str() {
            "--fps" => c.fps = Some(num()?),
            "--cost-ms" => c.cost_ms = num()?,
            "--hitch-ms" => c.hitch_ms = num()?,
            "--duration" => c.duration_s = num()?,
            "--log" => c.log = value.clone(),
            "--hitch-on-event" => {
                if value.is_empty()
                    || !value
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
                {
                    return Err("--hitch-on-event takes a plain name".into());
                }
                c.hitch_event = Some(value.clone());
            }
            "--hitch-at" => {
                c.hitch_at_s = value
                    .split(',')
                    .map(|s| s.trim().parse().map_err(|_| format!("bad hitch time: {s}")))
                    .collect::<Result<_, _>>()?;
                c.hitch_at_s.sort_by(f64::total_cmp);
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if c.fps.is_some_and(|f| f <= 0.0) || c.duration_s <= 0.0 {
        return Err("--fps and --duration must be positive".into());
    }
    Ok(c)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&config) {
        Ok(frames) => {
            println!("presented {frames} frames in {} s", config.duration_s);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn qpc() -> i64 {
    let mut t = 0i64;
    // SAFETY: `t` is a valid out-parameter; the call cannot fail on supported Windows.
    let _ = unsafe { QueryPerformanceCounter(&mut t) };
    t
}

/// Busy-waits until the QPC value `until`: a deliberate CPU burn, used for the
/// simulated frame cost, the planted hitches and the last stretch of the limiter.
fn spin_until(until: i64) {
    while qpc() < until {
        std::hint::spin_loop();
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        // SAFETY: plain message post on the window's own thread.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: forwards the unmodified message to the default handler.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: standard window creation; the class name and wndproc are 'static.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("VerityFramegen"),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("VerityFramegen"),
            w!("Verity framegen"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            720,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

fn run(c: &Config) -> Result<u64, Box<dyn std::error::Error>> {
    let hwnd = create_window()?;

    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ..Default::default()
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        OutputWindow: hwnd,
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let mut swap_chain: Option<IDXGISwapChain> = None;
    let mut device = None;
    let mut context = None;
    // SAFETY: all out-pointers are valid locals; `desc` outlives the call.
    // A null adapter picks the system default, which is what a game that does
    // not choose an adapter gets.
    unsafe {
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swap_chain),
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let (swap_chain, device, context) = match (swap_chain, device, context) {
        (Some(s), Some(d), Some(c)) => (s, d, c),
        _ => return Err("Direct3D returned no device or swap chain".into()),
    };
    let mut target = None;
    // SAFETY: buffer 0 exists for every swap chain; `target` is a valid out-pointer.
    unsafe {
        let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        device.CreateRenderTargetView(&back_buffer, None, Some(&mut target))?;
    }
    let target = target.ok_or("no render target view")?;

    let mut freq = 0i64;
    // SAFETY: `freq` is a valid out-parameter.
    let _ = unsafe { QueryPerformanceFrequency(&mut freq) };
    let ticks = |ms: f64| (ms / 1000.0 * freq as f64) as i64;

    let mut log = std::io::BufWriter::new(std::fs::File::create(&c.log)?);
    writeln!(log, "# qpc_freq={freq}")?;
    writeln!(log, "kind,qpc,ms")?;

    let start = qpc();
    writeln!(log, "start,{start},0")?;
    let end = start + ticks(c.duration_s * 1000.0);
    let period = c.fps.map(|fps| ticks(1000.0 / fps));
    let mut next_frame = start;
    let mut next_hitch = 0;
    let mut frames = 0u64;
    let hitch_event = match &c.hitch_event {
        Some(name) => {
            let full: Vec<u16> = format!(r"Local\{name}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            // SAFETY: `full` is NUL-terminated; auto-reset, initially unsignaled.
            Some(unsafe { CreateEventW(None, false, false, windows::core::PCWSTR(full.as_ptr())) }?)
        }
        None => None,
    };

    'frames: loop {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a valid out-parameter; messages are handled on this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'frames;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let now = qpc();
        if now >= end {
            break;
        }

        if let Some(&at) = c.hitch_at_s.get(next_hitch) {
            if now >= start + ticks(at * 1000.0) {
                writeln!(log, "hitch,{now},{}", c.hitch_ms)?;
                spin_until(now + ticks(c.hitch_ms));
                next_hitch += 1;
            }
        }
        if let Some(evt) = hitch_event {
            // SAFETY: `evt` is the event created above; a zero timeout only polls it.
            if unsafe { WaitForSingleObject(evt, 0) } == WAIT_OBJECT_0 {
                let now = qpc();
                writeln!(log, "hitch,{now},{}", c.hitch_ms)?;
                spin_until(now + ticks(c.hitch_ms));
            }
        }
        if c.cost_ms > 0.0 {
            spin_until(qpc() + ticks(c.cost_ms));
        }

        let shade = (frames % 120) as f32 / 120.0;
        // SAFETY: `target` and `swap_chain` are live COM objects owned by this thread.
        unsafe {
            context.ClearRenderTargetView(&target, &[0.05, shade * 0.4, 0.3, 1.0]);
            swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        frames += 1;

        if let Some(period) = period {
            next_frame += period;
            let now = qpc();
            if next_frame < now {
                // Fell behind (a hitch): restart the schedule instead of bursting to catch up.
                next_frame = now;
            } else {
                let sleep_ms = (next_frame - now) as f64 * 1000.0 / freq as f64 - 2.0;
                if sleep_ms > 0.0 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(sleep_ms / 1000.0));
                }
                spin_until(next_frame);
            }
        }
    }
    writeln!(log, "end,{},0", qpc())?;
    log.flush()?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn defaults_and_overrides() {
        let c = parse(&[]).unwrap();
        assert_eq!((c.fps, c.duration_s), (Some(60.0), 10.0));

        let c = parse(&args(
            "--uncapped --cost-ms 2.5 --hitch-at 3,1,2 --hitch-ms 80",
        ))
        .unwrap();
        assert_eq!(c.fps, None);
        assert_eq!(c.cost_ms, 2.5);
        assert_eq!(c.hitch_at_s, [1.0, 2.0, 3.0]);
        assert_eq!(c.hitch_ms, 80.0);
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!(parse(&args("--fps")).is_err());
        assert!(parse(&args("--fps zero")).is_err());
        assert!(parse(&args("--fps 0")).is_err());
        assert!(parse(&args("--nope 1")).is_err());
        assert!(parse(&args("--hitch-at 1,x")).is_err());
        assert!(parse(&args("--hitch-on-event bad/name")).is_err());
        assert_eq!(
            parse(&args("--hitch-on-event vt-1"))
                .unwrap()
                .hitch_event
                .as_deref(),
            Some("vt-1")
        );
    }
}
