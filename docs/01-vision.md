# 01 — Vision

## One line

Verity tells you exactly what is costing you frames, fixes it temporarily and reversibly, and proves the fix worked on your machine.

## The problem with existing optimizers

Most "game boosters" apply a fixed list of tweaks blindly. They do not know what is wrong with a particular machine, they do not measure before and after, and many of their tweaks do nothing or cause harm. The user ends up with a changed system and no evidence.

Good measurement tools exist separately: frame capture tools, DPC latency checkers, hardware monitors. None of them act on what they find, and none of them connect a stutter to its cause.

Verity closes the loop: **measure → attribute → act → verify**.

## The five pillars

### 1. Frame Story — stutter forensics
Every recorded session produces a timeline where each hitch is matched against what the system was doing at that instant: a driver's DPC spike, a background process, a Defender scan, a hard page fault, thermal throttling, a power source change. Verity reports causes with a statistical lift, not a guess. Spec: `07-frame-story.md`.

### 2. The Lab — proof by experiment
Any optimization can be A/B tested with a randomized block design and a real significance test. The verdict is one of: improves, hurts, no measurable effect, inconclusive. Only proven changes are kept. Spec: `08-lab.md`.

### 3. Doctor — real diagnostics, laptop-aware
A rule engine that checks for the things that actually cost large amounts of performance: the game running on the integrated GPU, a display stuck at 60 Hz, a charger that cannot keep up, thermal or power-limit throttling, a GPU-bound versus CPU-bound classification. Every finding carries numeric evidence. Spec: `04-doctor.md`.

### 4. Session Mode — temporary by default
When a game starts, Verity applies that game's profile. When the game exits, everything is put back. Background work is steered to efficiency cores and throttled with the scheduler's own mechanisms rather than killed. Spec: `06-session-mode.md`.

### 5. Total reversibility
Every change is journaled before it is made and can be reverted individually, per session, or all at once. A crash or power loss mid-session is repaired at next boot. Spec: `05-actions.md`, `09-safety.md`.

## What makes it different, concretely

| Typical optimizer | Verity |
|---|---|
| Applies a fixed tweak list | Diagnoses this machine, then proposes only relevant changes |
| No measurement | Frame-time capture on every session |
| "Up to 30% more FPS" | A measured effect with a confidence interval, or "no measurable effect" |
| Permanent registry edits | Session-scoped by default, journaled, auto-reverted |
| Kills processes and services | Throttles with EcoQoS and CPU sets; nothing is killed by default |
| Unknown anti-cheat risk | Never touches the game process; passive observation only |
| Cannot explain a stutter | Names the driver, process or event correlated with each hitch |

Honest framing: the individual operating-system levers Verity uses are documented Windows mechanisms. The product is the closed loop around them, the attribution engine, and the refusal to keep anything unproven.

## Target user and hardware

- A PC gamer, primarily on a gaming laptop, who wants smoother gameplay and wants to understand their machine
- Windows 10 22H2 or Windows 11, x64
- Hybrid graphics (integrated + discrete) and hybrid CPUs (performance + efficiency cores) are first-class cases

## Non-goals

- Overclocking, undervolting, fan control, BIOS changes. Verity may *recommend* a BIOS or vendor-app change with instructions, never perform it.
- An in-game overlay. Overlays need injection, which is banned. A separate window is acceptable.
- "Debloating" Windows, removing apps, disabling security features.
- Network "ping reducers".
- Driver updating.
- Cloud accounts or telemetry upload. All data stays on the machine.

## Product principles

1. Evidence beats folklore.
2. Temporary beats permanent.
3. Throttle beats kill.
4. Explain everything: every finding and action has a plain-language "what, why, risk, how to undo".
5. The user is in charge: nothing persistent is applied without explicit consent.
6. If the bottleneck is inside the game or the hardware, say so plainly instead of pretending a tweak will help.

## Product-level success criteria

- On a laptop with a planted fault (game forced onto the integrated GPU, or a CPU hog running in the background), Doctor or Frame Story identifies it by name.
- A Lab A/A test (same configuration in both arms) reports "improves" or "hurts" no more than 1 time in 20.
- Pulling the power mid-session and rebooting leaves the system in its pre-session state.
- Verity's own overhead during standard capture stays under 1% CPU on a modern 8-core machine.
