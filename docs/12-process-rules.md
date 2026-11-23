# 12 — Process Rules (M10)

Per-app CPU rules in the style of Process Lasso, with Verity's difference:
every rule is reversible through the journal, and every rule can be proven or
disproven in the Lab instead of being trusted on faith.

Items marked **(verify)** must be confirmed with a probe before being relied on.

## What Process Lasso does, and what Verity keeps

Research (Bitsum's documentation, 2026): Process Lasso offers persistent CPU
affinities, CPU sets ("softer" preferred cores), priority classes, I/O
priority, Windows 11 Efficiency Mode, ProBalance (temporarily lowers the
priority of background processes that monopolise the CPU), Performance Mode,
per-app power profiles, a Watchdog that acts on rules (change affinity or
priority, restart or terminate, trim memory), instance limits and disallowed
processes.

| Process Lasso feature | Verity | Why |
|---|---|---|
| Persistent CPU affinity per app | **Keep** | Documented API; real wins in specific cases (dual-CCD and X3D CPUs, hybrid CPUs) |
| CPU sets (preferred cores) | **Keep**, preferred over affinity | Softer: the scheduler can still use other cores under load |
| Priority class per app | **Keep, bounded** to Idle, Below normal, Normal, Above normal | High and Realtime are on the blacklist and never-do list (`09-safety.md`) |
| Efficiency Mode (EcoQoS) per app | **Keep** | Same mechanism as `sched.background_ecoqos` |
| Memory priority per app | **Keep** | Documented; protects the game's pages from a background hog |
| I/O priority per app | **Keep, undocumented path** | Only via `NtSetInformationProcess`; goes in `verity-sys::undocumented` |
| ProBalance (dynamic restraint) | **Keep** as "Restraint" | Extends the existing background actions with a CPU-hog trigger |
| Per-app power profile | Already planned | Session Mode triggered by any app (decision 2026-11-14) |
| Terminate, instance limits, disallowed processes | **Drop** | Never-do: kills processes outside the session-pause allowlist |
| SmartTrim / memory trimming | **Drop** | Blacklist: RAM cleaners cause the stutter they claim to fix |
| CPU limiter (cap a process's CPU) | **Lab only** | A job object cannot be left once joined, so the exact prior state is not restorable |

## Rules

A rule is configuration stored in Verity's database. Applying it is a
process-targeted, session-scoped action (`05-actions.md`), journaled per
process.

```
rule {
  id, enabled,
  match:   exe path (preferred) or exe file name,
  when:    always | while Session Mode is active | while a chosen game runs,
  cpu:     none | cpu_set(ids) | affinity(mask) | preset(name),
  priority: none | idle | below_normal | normal | above_normal,
  efficiency: none | on | off,
  memory_priority: none | very_low | low | medium | below_normal | normal,
  io_priority: none | very_low | low | normal,     // undocumented, see below
  proven_by: lab experiment id or null
}
```

- The service applies a rule when a matching process starts (it already watches
  Microsoft-Windows-Kernel-Process for Session Mode) and to matching processes
  that are running when the rule is enabled.
- Disabling or deleting a rule reverts it on every live process it touched.
- Panic, service stop, suspend and crash recovery revert rule applications
  exactly like other session-scoped actions.
- Each apply records `(pid, creation time, image name)` and the prior value of
  every setting it changed. Revert touches only a process whose identity still
  matches, and marks `superseded` when the user or another tool changed the
  value since.
- `verify()` runs every 10 s for live applications and reports drift in the UI.

### Presets from the CPU's real layout

`GetLogicalProcessorInformationEx` and `GetSystemCpuSetInformation` give the
topology. Presets are offered only when the topology has them:

| Preset | Shown when | Meaning |
|---|---|---|
| Performance cores | Hybrid CPU (more than one `EfficiencyClass`) | CPU sets of the highest efficiency class |
| Efficiency cores | Hybrid CPU | CPU sets of the lowest efficiency class |
| Cache-heavy CCD | Two L3 domains of different size (AMD X3D) **(verify)** | Cores sharing the largest L3 |
| Frequency CCD | Same | Cores of the other L3 domain |
| One CCD | Two or more L3 domains | Cores of the first L3 domain |
| One thread per core | SMT enabled | First logical processor of each core |

### Mechanisms

| Setting | API | Rights on the target | Snapshot |
|---|---|---|---|
| CPU set | `SetProcessDefaultCpuSets`, or `SetProcessDefaultCpuSetMasks` on Windows 11 for more than 64 processors | `PROCESS_SET_LIMITED_INFORMATION` | `GetProcessDefaultCpuSets` |
| Affinity | `SetProcessAffinityMask` (single processor group only; otherwise use a CPU set) | `PROCESS_SET_INFORMATION` | `GetProcessAffinityMask` |
| Priority class | `SetPriorityClass`, four allowed values | `PROCESS_SET_INFORMATION` | `GetPriorityClass` |
| Efficiency | `SetProcessInformation(ProcessPowerThrottling)` | `PROCESS_SET_INFORMATION` | `GetProcessInformation` |
| Memory priority | `SetProcessInformation(ProcessMemoryPriority)` **(verify)** | `PROCESS_SET_INFORMATION` | `GetProcessInformation` |
| I/O priority | `NtSetInformationProcess(ProcessIoPriority)` **(verify)**, behind the `undocumented` feature with a capability check; not offered when unavailable | `PROCESS_SET_INFORMATION` | `NtQueryInformationProcess` |

Handles are opened with exactly the rights in the table plus
`PROCESS_QUERY_LIMITED_INFORMATION`, never `PROCESS_VM_*`, and closed at once.
Access denied (protected processes, anti-cheat) is a normal result, shown as
"Windows does not allow changing this app".

## Restraint (ProBalance-style)

A session-scoped background action, off by default until the Lab shows a gain
on this machine.

- Trigger: total CPU above 70% for 2 s, and a process outside the exclusion list
  using more than 15% of all cores for that time, while a game or the
  foreground app is active. Thresholds are starting values, tuned in the Lab.
- Action: Below normal priority plus Efficiency Mode on that process.
- Release: its use stays under 5% for 5 s, or it comes to the foreground.
- Every restraint is journaled, shown in the Processes screen's log and in
  Frame Story's timeline, and reverted at session end.

## Games

A rule may target a library game only with these settings, and only when the
game is not in strict passive mode (`09-safety.md` §2):

- a CPU set or preset (soft steering), Lab first
- Above normal priority (the existing `sched.game_priority`), Lab first

Hard affinity, efficiency, memory or I/O priority on a game stay Lab-only
experiments (`sched.game_cpusets`). In strict passive mode a game rule is
shown as paused, with the reason.

## Exclusions

Rules never apply to: session 0 processes, protected processes, anti-cheat and
security software, `csrss.exe`, `dwm.exe`, `audiodg.exe`, `winlogon.exe`,
Verity itself. A user rule may target any other app the user picks, including
foreground apps; automatic Restraint follows the full background exclusion
list in `05-actions.md`.

## Proving a rule

"Prove it" starts a Lab experiment (`08-lab.md`): rule off vs rule on,
interleaved blocks.

- For a game: frame-time metrics, as for every Lab experiment.
- For another app: the user runs a repeatable task (a render, an export, a
  compile) and marks start and end, or picks a metric Verity can read without
  touching the app (its CPU time per block, the foreground's frame pacing).
- The verdict is Improves, Hurts, or no measurable effect. A rule without a
  verdict is labelled "not proven" everywhere it appears.

## IPC

| Method | Who | Notes |
|---|---|---|
| `processes.live` | any client | pid, image name, CPU %, priority class, CPU set summary, rule id. Names only: no command lines, no window titles. |
| `topology.get` | any client | cores, efficiency classes, L3 domains, presets |
| `rules.list`, `rules.get` | any client | |
| `rules.set`, `rules.delete`, `rules.enable` | any client for its own session's apps; machine-wide rules need an elevated client | Parameters validated against the rule schema; exe paths are matched, never opened, run or written |
| `restraint.log` | any client | recent restraints |

## UI: the Processes screen

- Live list: name, CPU %, cores in use, priority, rule badge, "Windows does not
  allow" marker. Sorted by CPU by default.
- Core map: one tile per logical processor, grouped by P/E or CCD, coloured by
  load; selecting a process highlights where it may run.
- Rule editor: presets first, raw core picker second; each setting shows what
  it does, its downside and how to undo.
- Restraint log, and drift notices.

## Amendments to `09-safety.md` (approved by the owner, 2026-11-23)

1. §2 Allowed: open a **non-game** process with the rights in the Mechanisms
   table, only to apply or revert a user rule or a background action.
2. §2 Allowed: open a game with `PROCESS_SET_LIMITED_INFORMATION` for a CPU set
   rule, never in strict passive mode.
3. §3 stays: no High or Realtime priority for any process, no terminating.

## Dependencies

M3 (action framework, journal, exclusion list), M4 (process watcher), M7 (Lab
verdicts). M10 starts when M3 is merged.
