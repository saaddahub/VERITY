# Verity: project memory

**Read this first in every new chat.** It says where the project stands and what
is next. Then follow `CLAUDE.md` (or `AGENTS.md` for Codex). Details live in
`docs/notes/STATUS.md` (what works, with evidence) and `docs/notes/decisions.md`
(every departure from the spec, with the reason).

**Keep it current:** at the end of every session, update "Last session",
"Done" and "Next" below, then commit it with the work.

Last updated: 2026-11-26

## What Verity is

A Windows performance tool for games and everyday use: it measures, finds the
cause, changes things reversibly, and proves the change helped. Rust service
(LocalSystem) + `verity` CLI + Tauri desktop app, talking over a named pipe.
Spec: `docs/01-vision.md` to `docs/11-roadmap.md`. Who it is for: `PRODUCT.md`.

## Owner's standing preferences

- **GUI first.** From 2026-11-16, features are built into the desktop app
  (`apps/verity-ui`) and verified there; the CLI stays as a second face.
- **UI work uses the full UI skill set** (impeccable, interface-design,
  apple-design, the better-* passes, variant, state-machine, break). Never just one.
- **Plain language.** The owner is not a developer by trade; explain outcomes
  simply and avoid jargon.
- Figma is on the Starter plan: the MCP call limit runs out after a few calls.
- Test only on the owner's laptop until M9. No games with kernel anti-cheat.

## Done

- **M0, skeleton** (complete 2026-11-14): service install/start/stop/uninstall,
  secured named pipe, system info. Verified elevated.
- **M1, telemetry** (complete 2026-11-16): PresentMon 2.6.0 capture, FPS, 1% and
  0.1% lows, stutter (hitch) detection, GPU attribution, SQLite sessions, CSV
  export. Real-game check: Forza Horizon 5, 68.0 FPS average / 40.1 1% low,
  matching the owner's in-game counter.
- **M3 Actions and journal, complete** (2026-11-26, built here because the
  co-developer's M3 never reached GitHub; owner's call). Seven actions (power
  mode, efficiency mode, efficiency cores, GPU preference, max refresh, Game
  Mode, startup items), journal written before every change, crash recovery
  at service start, exclusion list, restore point before the first lasting
  change, `verity actions` / `verity journal` CLI. Session Mode now really
  applies and undoes its changes. Admin tests: `tools/m3-admin.ps1`.
- **Desktop app v0** (merged to `main` via PR #1, 2026-11-16): Checkup, Sessions,
  Record, one-prompt service setup. Design "Workshop Inspection Report",
  contract in `.impeccable/surfaces/apps-verity-ui.md`.
- **M4 Session Mode and M5 Frame Story, built** (2026-11-17, branch
  `m4-m5-session-story`, not merged, not pushed): game library (finds the 3
  Steam games on the laptop), automatic recording of each game session, panic
  hotkey Ctrl+Alt+Shift+V and tray, Games screen; Frame Story matches each
  stutter to driver work, disk, memory, background CPU/GPU, CPU limits,
  charger, Defender, Windows Update and program starts, with lift and
  significance; shareable HTML report. 5 admin end-to-end tests pass
  (`tools/m4m5-admin.ps1`): planted cause found 20 of 20, decoy not reported,
  panic in 1.8 ms, overhead 0.29% of a 1% budget. Details in
  `docs/notes/STATUS.md`; departures from the spec in `docs/notes/decisions.md`.
- **Scope broadened** to everyday PC use (`docs/01-vision.md` pillar 6).
- Project report PDF for the co-developer: `docs/report/` (kept local, not in git).

## Next (in order)

0. **M12 Storage is built** (2026-11-24, see `docs/notes/STATUS.md`): owner's
   manual check left (free space with the suggestions, restore one file). Next:
   **M13 Malware Scan** (`docs/15-malware-scan.md`), reusing the WMI wrapper in
   `verity-sys::wmi`. M10 Process Rules and M11 Network Control wait for M3.
   Safety amendments for all four are approved in `docs/09-safety.md`.
1. **M2 Doctor is still the co-developer's** (not on GitHub). M3 is done
   here: if they push their own M3, reconcile with `crates/verity-actions`
   rather than keeping two. Then add Frame Story rules D030–D034 to M2.
2. **Owner's manual checks for M4/M5:** sleep the laptop during a session;
   play a real game 15 minutes with Session Mode on (audio, voice chat, capture
   software normal); open that session's Frame Story and judge its top cause.
3. **Finish the app's design review.** Retake two 375-px screenshots (Checkup and
   Sessions #4) at 2x into `.impeccable/review/`, include the new Games screen
   and Frame Story panel, then write `DESIGN.md` with the impeccable documenter.
4. **Record the "not fully offline" decision** (owner, 2026-11-16) in
   `docs/09-safety.md`, `docs/01-vision.md`, `PRODUCT.md` (it still says "Fully
   offline") and `docs/notes/decisions.md`.
5. **FreeLLMAPI fallback for Claude Code.** Waiting on the owner to install the
   FreeLLMAPI router and add free provider keys.
6. **Verity chat assistant** in the app, powered by FreeLLMAPI (owner request).
7. **Desktop smoothness** (`record --desktop`): `dwm.exe` has long idle gaps, so
   it needs its own metric before it can ship.

## Open questions for the owner

- Repo is public; license is Apache 2.0 (from GitHub's checkbox). Confirm both.
- Business model and distribution are undecided (see the report, section 11).

## How to run things

- Checks: `.\tools\check.ps1` (add `-Admin` in an elevated terminal for service tests).
- App: `cd apps\verity-ui; npm run tauri dev` (the service must be installed and running).
- UI preview in a browser with sample data: `npm --prefix apps/verity-ui run dev`, open `http://localhost:1420`.
- PresentMon is not in git: see `tools/presentmon/README.md`.

## Last session (2026-11-23 to 24)

Code review of the whole codebase: 10 findings fixed (oversized IPC replies,
relaunch capture, kernel event growth, library sync deleting profiles, manual
start identity, and smaller ones); chart axis fix. The owner asked for Process
Lasso, NetLimiter, storage and antivirus features: researched and specified as
M10 to M13 with owner-approved safety amendments (no kernel driver; Recycle
Bin plus confirmed permanent delete; CPU rules on non-game apps). Built M12
Storage end to end and checked it in the real app. Then, at the owner's request, Storage became PC Manager-style: one-click junk cleanup (Windows Disk Cleanup handlers in the service, user caches in the app), nothing opens Explorer or Settings. The owner's M4/M5 branch
was merged to `main` as PR #2; later commits on `m4-m5-session-story` are not
pushed yet.
