# Verity — spec pack for Claude Code

**Verity** is a Windows game-performance optimizer that measures first, changes second, and proves every change on *your* machine. This folder is the complete brief for Claude Code to build it.

## What is in this pack

| File | What it is |
|---|---|
| `CLAUDE.md` | Standing instructions Claude Code loads every session: rules, stack, commands, code standards. |
| `docs/01-vision.md` | What Verity is, why it is different, what it will never do. |
| `docs/02-architecture.md` | Processes, crates, IPC, storage schema, timekeeping. |
| `docs/03-telemetry.md` | The measurement engine: frame pacing, system counters, kernel tracing. |
| `docs/04-doctor.md` | Diagnostic rules that find real bottlenecks (laptop-aware). |
| `docs/05-actions.md` | The catalog of optimizations and the reversible Action framework. |
| `docs/06-session-mode.md` | Game detection, per-game profiles, auto-apply and auto-revert. |
| `docs/07-frame-story.md` | Stutter forensics: which system event caused which hitch. |
| `docs/08-lab.md` | A/B experiment engine with real statistics. |
| `docs/09-safety.md` | Reversibility, anti-cheat rules, the snake-oil blacklist. |
| `docs/10-ui.md` | Screens, tray behavior, design direction. |
| `docs/11-roadmap.md` | Milestones M0–M9 with acceptance criteria, plus the test strategy. |

## Setup on the Windows laptop

1. Install:
   - Git
   - Rust via rustup (stable, `x86_64-pc-windows-msvc`)
   - Visual Studio Build Tools 2022 with the "Desktop development with C++" workload
   - Node.js LTS (needed from M6 for the UI)
   - Claude Code
2. Create an empty folder named `verity`. Copy `CLAUDE.md` to its root and the `docs/` folder next to it.
3. In that folder: `git init`, then commit the spec pack as the first commit.
4. Download the PresentMon console application from the GameTechDev/PresentMon GitHub releases and place the exe in `tools/presentmon/`. Keep its license file beside it.
5. Open **two** terminals in the folder: a normal one for Claude Code, and an elevated (Run as administrator) one for the admin-only integration tests.

No Windows "API key" is needed. The Windows API is a set of system libraries; the program needs administrator rights for the privileged parts, nothing else.

## How to drive Claude Code

Work one milestone per session. Start each session in plan mode so you approve the plan before code is written.

First session:

```
Read CLAUDE.md, then docs/01-vision.md, docs/02-architecture.md, docs/09-safety.md and docs/11-roadmap.md.
Summarize the product and the rules back to me in 10 bullets, list anything in the specs you think is wrong or risky,
then propose a plan for Milestone 0. Do not write code until I approve the plan.
```

Every later session:

```
Read CLAUDE.md and docs/11-roadmap.md. We are on Milestone <N>. Read the spec files that milestone references.
Propose a plan, wait for my approval, then implement it. Finish by running the full check suite and walking me
through each acceptance criterion with evidence.
```

When something behaves differently on the real machine than the spec says:

```
The spec says <X> but on my machine <Y> happens. Write a probe in tools/probes to confirm, fix the code to match
reality, then update the spec and add an entry to docs/notes/decisions.md.
```

## Ground rules for you, the human

- Commit after every milestone. If a session goes sideways, `git reset` is your undo.
- Test on your own laptop only until M9. The software changes system settings; everything is designed to be reversible, but early builds will have bugs.
- Create a Windows restore point before the first time you run a build that applies actions (M3).
- Never test against games with kernel anti-cheat (Valorant, etc.) until the safety checklist in `docs/09-safety.md` passes.
