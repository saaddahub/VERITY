# 04 — Doctor

A rule engine that finds what is actually wrong. Each rule is small, testable with fake inputs, and produces a finding with numbers in it.

## Rule interface

```rust
pub trait Rule {
    fn id(&self) -> &'static str;                  // "D003"
    fn kind(&self) -> RuleKind;                    // Static | Session
    fn evaluate(&self, ctx: &DoctorContext) -> Option<Finding>;
}

pub struct Finding {
    pub rule_id: &'static str,
    pub severity: Severity,        // Critical | High | Medium | Info
    pub confidence: Confidence,    // High | Medium | Low
    pub title: String,             // plain language
    pub evidence: Vec<Evidence>,   // named numbers with units
    pub explanation: String,       // why it matters
    pub remedy: Remedy,            // Action(ActionRef) | Guide(steps) | LabSuggestion | None
}
```

- **Static** rules read current system state and can run any time.
- **Session** rules need a recorded session (or several) and read its summary, samples and events.
- Severity reflects the expected size of the effect. Confidence reflects how sure the detection is. Never raise severity to look impressive.
- A finding without numeric evidence is not allowed.

## Rule catalog

Thresholds are config values; the numbers below are defaults.

### Graphics path

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D001 | Game is rendering on the integrated GPU | Session: the game pid's GPU Engine 3D utilization is on the integrated adapter's LUID while a discrete adapter exists | Action `gpu.preference` |
| D002 | No per-app GPU preference set on a hybrid-graphics machine | Static: discrete + integrated adapters present and no entry for the game exe | Action `gpu.preference` |
| D003 | Display is running below its maximum refresh rate | Static (agent): current refresh < highest mode at the current resolution | Action `display.max_refresh` |
| D004 | Frames pass through the integrated GPU to reach the display | Static: the internal panel is attached to the integrated adapter while games render on the discrete one | Guide: check the vendor app or BIOS for a discrete-only / MUX mode. Info severity. |
| D005 | Windowed-game optimizations or VRR setting is off | Static: values in the DirectX user global settings | Action `dx.global_settings`, Low confidence |
| D006 | GPU driver is old | Static: driver date older than 9 months | Guide only. Verity does not update drivers. |
| D007 | VRAM is exhausted | Session: game's dedicated usage ≥ 95% of adapter dedicated memory for > 10% of the session | Guide: lower texture quality |

### Power and thermals

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D010 | Gaming on battery | Static/Session: power source is battery during a session | Guide: plug in. Critical on laptops. |
| D011 | Charger cannot keep up | Session: on AC, battery reports discharging with a negative rate for > 60 s under load | Guide: use the original or a higher-wattage charger; USB-C chargers are a common cause |
| D012 | Power mode limits performance | Static: active overlay is not best-performance, or scheme is a power saver, while on AC | Action `power.session_overlay` |
| D013 | CPU is being throttled | Session: `% Performance Limit` < 100 or limit flags set for > 10% of the session | Guide: cooling, vendor performance mode, cleaning. Report the share of time and the average limit. |
| D014 | GPU is thermally limited | Session: GPU temperature (if available) near its limit with falling utilization and rising frame time | Guide. Low confidence unless the temperature source is verified. |
| D015 | Vendor performance profile is probably set to quiet/balanced | Session: on AC, CPU performance limit engages while GPU and CPU are both far from full load | Guide: check the laptop vendor's control app. Low confidence. |

### CPU, memory, storage

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D020 | Background processes are taking CPU during play | Session: non-game, non-system processes sum > 10% of total CPU averaged over the session, or any single one > 5% | Actions `sched.background_ecoqos`, `sched.background_cpusets`; list offenders by name with their share |
| D021 | Memory pressure | Session: available memory < 10% of total, with sustained hard faults in the game | Guide: close named top memory users. Offer `memory.standby_purge` only if standby cache is large. |
| D022 | RAM is running below its rated speed | Static: `ConfiguredClockSpeed` < `Speed` on any module | Guide: enable the memory profile in BIOS. Medium confidence. |
| D023 | RAM appears to be single-channel | Static: one populated module | Guide. Low confidence; note the large effect on integrated graphics. |
| D024 | Game is installed on a hard disk | Static: game volume reports a seek penalty | Guide: move to SSD |
| D025 | System or game drive is nearly full | Static: free space < 10% | Guide |
| D026 | Many startup programs | Static: count and list of enabled startup items | Action `startup.disable_item`, per item, user picks |

### System activity during play

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D030 | A driver is causing latency spikes | Session (Standard): a driver's DPC/ISR time has events > 1 ms, or it appears as a Frame Story cause | Guide per driver class (network, audio, GPU, storage): update driver, try disabling the related device feature. Name the driver file. |
| D031 | Security scan ran during play | Session: Defender scan event overlaps the session | Guide: schedule scans outside play hours. Never offer to disable Defender. |
| D032 | Windows Update was active during play | Session: update download/install events overlap | Guide: set active hours |
| D033 | Background disk or network activity | Session: a non-game process moved > 50 MB/min of disk or network I/O | Name it; guide to pause its sync or download |
| D034 | Overlays are loaded into the game | Session (Standard): known overlay modules appear in the game's image-load events | Info: list them. The Lab can test with them disabled. |

### Windows settings

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D040 | Game Mode is off | Static (agent) | Action `gamemode.enable` |
| D041 | Hardware-accelerated GPU scheduling state | Static | Info only. Suggest a Lab experiment; never recommend blindly. |
| D042 | Memory integrity / virtualization-based security is on | Static | Info only: describes the trade-off. Verity never changes it. |

### Bottleneck

| ID | Finding | Detection | Remedy |
|---|---|---|---|
| D050 | The game is GPU-bound | Session: GPU-bound share > 70% | Guide: resolution scale, upscaler, shadows. State that system tweaks will have little effect. |
| D051 | The game is CPU-bound | Session: CPU-bound share > 50% | Prioritize D020, D013, scheduling actions; guide on CPU-heavy settings |
| D052 | A frame cap is limiting FPS | Session: capped share > 70% | Info: state the cap value. This is fine if intended. |

## Ordering and presentation

Sort by severity, then confidence. Show at most the top five by default with the rest collapsed. Each finding shows: title, the evidence numbers, a two-sentence explanation, and its remedy with a risk label.

Findings that are true but harmless on this machine should not be created. Example: do not emit D002 on a desktop with one GPU.

## Cross-session rules

Session rules may aggregate over the last N sessions of the same game. Where they do, the evidence states the sample ("in 7 of your last 10 sessions").

## Verified fixes

When a finding's remedy was applied, later sessions re-evaluate the rule. If it no longer fires, the finding is marked **verified fixed** with the before and after numbers. If it still fires, say so.

## CLI

```
verity doctor                 # static rules
verity doctor --session <id>  # static + session rules
verity doctor --json
```

## Tests

Every rule has unit tests with a fake `DoctorContext`: one case that fires, one near-miss that does not, one with missing data (must not fire and must not panic).
