# 03 — Telemetry

The measurement engine. Everything else in Verity depends on this being accurate and cheap.

Items marked **(verify)** must be confirmed with a probe before being relied on.

## Capture modes

| Mode | Contains | When |
|---|---|---|
| Light | Frames, 1 Hz system counters, process start/stop, event-log watchers | Every session, always |
| Standard | Light + DPC/ISR, hard faults, disk I/O, image loads, 4 Hz counters | Default for recorded sessions |
| Deep | Standard + context switches | On demand, time-limited to 5 minutes, for hard cases |

Overhead budget on an 8-core machine: Light < 0.3% CPU, Standard < 1%, Deep < 3%. The service measures its own CPU time per session and stores it in the summary. A test fails if the budget is exceeded.

## 1. Frame pacing

### v1: PresentMon as a child process (M1)

Use the open-source PresentMon console application as the frame source. It consumes the DXGI, D3D9 and DxgKrnl ETW providers and emits one CSV row per presented frame. This observes the game passively; no handle to the game process is opened.

- Launch from the service (SYSTEM), targeting the game by process id, writing CSV to stdout, with QPC timestamps, a private session name (`VerityPM`), stop-existing-session, and terminate-on-process-exit. Verified for the pinned PresentMon 2.6.0: `--process_id <pid> --output_stdout --qpc_time --v2_metrics --session_name VerityPM --stop_existing_session --terminate_on_proc_exit --no_console_stats`. Capturing needs administrator rights.
- Redirected stdout is UTF-16 LE with a byte-order mark; decode before parsing.
- Parse the header row dynamically and map columns by name. Fail loudly on an unknown layout.
- Columns in 2.6.0 (verified): `Application, ProcessID, SwapChainAddress, PresentRuntime, SyncInterval, PresentFlags, AllowsTearing, PresentMode, CPUStartQPC, FrameTime, CPUBusy, CPUWait, GPULatency, GPUTime, GPUBusy, GPUWait, DisplayLatency, DisplayedTime, AnimationError, AnimationTime, MsFlipDelay, AllInputToPhotonLatency, ClickToPhotonLatency`. A missing value is the text `NA`.
- If PresentMon exits unexpectedly, restart it once, record the gap as an event of kind `capture_gap`, and never count the gap as a hitch.
- Ship its license text in the installer and the About screen.

### v2: native ETW consumer (M8, optional)

Replace the child process with an in-process consumer of the same providers. Only worth doing if v1 shows real limitations. The PresentMon source is the reference for the event state machine.

### Frame record

```rust
struct Frame {
    qpc: u64,                 // CPU start of the frame
    frame_time_ms: f32,       // CPU start to next CPU start
    cpu_busy_ms: f32,
    gpu_busy_ms: f32,
    displayed_time_ms: f32,   // NaN if not displayed
    display_latency_ms: f32,  // NaN if unknown
    present_mode: u8,
    flags: u8,                // dropped, tearing allowed, ...
}
```

When several swap chains exist, the primary one is the swap chain with the most displayed frames over the last 5 seconds.

## 2. System counters (PDH)

Sample at 1 Hz (Light) or 4 Hz (Standard). Add all counters with `PdhAddEnglishCounterW`. Wildcard instances are re-expanded every 5 seconds.

| Metric | Counter |
|---|---|
| CPU total and per core | `\Processor Information(*)\% Processor Utility` |
| CPU frequency state | `\Processor Information(_Total)\% Processor Performance` |
| Throttling | `\Processor Information(_Total)\% Performance Limit`, `\Processor Information(_Total)\Performance Limit Flags` **(verify)** flag meanings |
| Per-process CPU | `\Process V2(*)\% Processor Time` on Windows 11; fall back to `\Process(*)` plus pid mapping on Windows 10 |
| Memory | `\Memory\Available MBytes`, `\Memory\Committed Bytes`, `\Memory\Pages Input/sec`, `\Memory\Standby Cache Normal Priority Bytes` |
| Disk | `\PhysicalDisk(*)\% Idle Time`, `\PhysicalDisk(*)\Avg. Disk Queue Length`, `\PhysicalDisk(*)\Disk Bytes/sec` |
| GPU engines | `\GPU Engine(*)\Utilization Percentage` |
| GPU memory | `\GPU Adapter Memory(*)\Dedicated Usage`, `\GPU Process Memory(*)\Dedicated Usage` |
| Network | `\Network Interface(*)\Bytes Total/sec` |

GPU Engine instance names have the form `pid_<pid>_luid_<high>_<low>_phys_<n>_eng_<n>_engtype_<type>`. Parse pid and LUID. Map LUID to adapter through DXGI `EnumAdapters1` (`DXGI_ADAPTER_DESC1.AdapterLuid`). This is how Verity knows **which GPU a game is actually rendering on** without touching the game.

## 3. Hardware and power state

| Data | Source |
|---|---|
| CPU topology, P-core / E-core map | `GetSystemCpuSetInformation` (`EfficiencyClass`), `GetLogicalProcessorInformationEx` |
| GPU adapters, VRAM, vendor | DXGI `EnumAdapters1` |
| Which adapter drives each display | DXGI `EnumOutputs` per adapter |
| GPU temperature, power, fan | `D3DKMTQueryAdapterInfo` with the adapter performance-data query **(verify)** struct layout and units with a probe; treat as optional |
| AC / battery | `GetSystemPowerStatus`, plus `RegisterPowerSettingNotification` for `GUID_ACDC_POWER_SOURCE` |
| Battery charge/discharge rate | Battery device via SetupDi (`GUID_DEVCLASS_BATTERY`), `IOCTL_BATTERY_QUERY_TAG`, then `IOCTL_BATTERY_QUERY_STATUS` (`Rate` in mW, `PowerState` flags) |
| Active power scheme and overlay | `PowerGetActiveScheme`; overlay via the undocumented `PowerGetEffectiveOverlayScheme` **(verify)** |
| Display mode and maximum refresh | `EnumDisplaySettingsExW`, `QueryDisplayConfig` (agent side) |
| RAM modules, rated vs configured speed | WMI `Win32_PhysicalMemory` (`Speed`, `ConfiguredClockSpeed`, slot info) |
| Storage type per volume | `IOCTL_STORAGE_QUERY_PROPERTY` (seek penalty, bus type), free space |

**CPU temperature:** there is no reliable driverless source on Windows. Verity does not show CPU temperature in v1. It shows the *consequence* instead: the performance-limit counters above reveal thermal and power-limit throttling directly. Never bundle WinRing0 or any third-party kernel driver (see `09-safety.md`).

## 4. Kernel tracing (Standard and Deep)

One private system trace session named `VerityKernel`, using the system logger mode available since Windows 8 (`EVENT_TRACE_SYSTEM_LOGGER_MODE`) so Verity does not fight other tools for the single legacy "NT Kernel Logger". QPC clock. Real-time mode, no file.

The session runs all the time with only the `PROCESS` flag, which is what Session Mode uses to see games start (`06-session-mode.md`). When a Standard or Deep capture begins, the extra flags are switched on by updating the running session (`ControlTraceW` with `EVENT_TRACE_CONTROL_UPDATE`) **(verify)**, and switched off again when the capture ends. If updating flags in place does not work, stop and restart the session with the new flag set.

| Flag | Used for |
|---|---|
| `PROCESS` | process start/stop with image name; command lines are discarded, never stored |
| `IMAGE_LOAD` | module loads into the game (overlay detection, passively) and the kernel module map |
| `DPC`, `INTERRUPT` | DPC and ISR durations with routine address |
| `MEMORY_HARD_FAULTS` | hard page faults by process |
| `DISK_IO` | disk read/write latency and size by process |
| `CSWITCH` (Deep only) | who ran on which core when; ready-thread contention for the game |

DPC/ISR decoding **(verify)** opcodes: the PerfInfo event class carries ISR, DPC, timer DPC and threaded DPC events with an initial time and a routine address. Duration = event timestamp − initial time. Attribute the routine address to a driver using the kernel module map built from image-load rundown events (base address, size, file name); fall back to `EnumDeviceDrivers`.

To keep volume low: in the callback, aggregate DPC/ISR into 10 ms buckets per driver (count, total, max) and additionally emit an individual event for any single DPC or ISR longer than 500 µs.

No stack walking in v1.

## 5. Event-log watchers

Subscribe with `EvtSubscribe` during sessions **(verify)** channel names and event ids on the machine:

- Windows Defender operational log: scan started / finished
- Windows Update client operational log: download and install started / finished
- Task Scheduler operational log: task started / completed. This log is often disabled; if so, report "scheduled task visibility unavailable" as a capability, do not enable it silently.

Each becomes an `events` row with a start and, where known, a duration.

## 6. Derived metrics

All defined once in `verity-core::metrics` and used everywhere.

| Metric | Definition |
|---|---|
| Average FPS | frame count / duration |
| 1% low FPS | 1000 / (99th percentile of frame time in ms) |
| 0.1% low FPS | 1000 / (99.9th percentile of frame time in ms) |
| Rolling median | median frame time over the previous 120 frames |
| Hitch | a frame with `frame_time > 2 × rolling median` **and** `frame_time > rolling median + 4 ms` |
| Hitch group | hitches less than 500 ms apart merged into one |
| Hitch rate | hitch groups per minute |
| Pacing spread | interquartile range of frame time |
| Bottleneck | per 1 s window: **GPU-bound** if median `gpu_busy ≥ 0.90 × frame_time`; **CPU-bound** if `gpu_busy ≤ 0.75 × frame_time` and not capped; **capped** if frame time sits within ±3% of a refresh-rate divisor or a constant for the window |

Percentiles use frame time. (The displayed-time series was tried first and dropped: when the game window is hidden most frames are never displayed and the rest stay up for seconds, so the lows collapse. See `docs/notes/decisions.md`.)

Exclude from all metrics: the first 5 seconds after capture start, capture gaps, and periods where the game is not the foreground window (the agent supplies foreground changes as events).

## 7. Session summary

Stored as `summary_json`: duration, frame count, all derived metrics, bottleneck time shares, GPU adapter used, power source timeline, throttled time share, capture mode, dropped-event counts, Verity overhead. This is what lists and comparisons read; chunks are loaded only for the timeline view.

## 8. CLI surface for this subsystem

```
verity record --pid <pid> | --exe <name> [--mode light|standard|deep] [--duration 120]
verity sessions list
verity sessions show <id> [--json]
verity sessions export <id> --csv <path>
verity system info
```
