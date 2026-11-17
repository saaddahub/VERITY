# Verity: project memory

**Read this first in every new chat.** It says where the project stands and what
is next. Then follow `CLAUDE.md` (or `AGENTS.md` for Codex). Details live in
`docs/notes/STATUS.md` (what works, with evidence) and `docs/notes/decisions.md`
(every departure from the spec, with the reason).

**Keep it current:** at the end of every session, update "Last session",
"Done" and "Next" below, then commit it with the work.

Last updated: 2026-11-17

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
- **Desktop app v0** (merged to `main` via PR #1, 2026-11-16): Checkup (verdict +
  garage-style inspection sheet), Sessions (frame-time trace with stutter
  marks), Record (pick an app using the GPU), Install/Start service with one
  admin prompt, compact layout. Design: "Workshop Inspection Report", contract
  in `.impeccable/surfaces/apps-verity-ui.md`. The finish reviewer's 8 fixes are
  applied.
- **Scope broadened** to everyday PC use (`docs/01-vision.md` pillar 6).
- Project report PDF for the co-developer: `docs/report/` (kept local, not in git).

## Next (in order)

1. **Finish the app's design review.** The reviewer still has to sign off the
   8 fixes: retake two 375-px-wide screenshots (Checkup and Sessions #4) at 2x
   into `.impeccable/review/`, send all six captures back, then write
   `DESIGN.md` with the impeccable documenter.
2. **Record the "not fully offline" decision.** On 2026-11-16 the owner said
   Verity is not meant to be fully offline. Update `docs/09-safety.md`,
   `docs/01-vision.md` (non-goals) and `PRODUCT.md`, and add an entry to
   `docs/notes/decisions.md`.
3. **FreeLLMAPI fallback for Claude Code.** Waiting on the owner: install the
   FreeLLMAPI router (Microsoft Store app, serves `http://localhost:3001`) and
   add free provider keys. Then: install the `claude` CLI and launch it with
   `npx freellmapi launch`; optionally add the router's `/mcp` as an MCP tool.
4. **Verity chat assistant** in the app, powered by FreeLLMAPI (owner request).
5. **M2, Doctor**: diagnostic rules (throttling, charger, background load, power
   mode), written for any PC. Includes bottleneck classification. The UI's
   quick checks in `apps/verity-ui/src/checks.ts` move into the service then.
6. **Desktop smoothness** (`record --desktop`): the probe showed `dwm.exe` has
   long idle gaps, so it needs its own metric before it can ship.

## Open questions for the owner

- Repo is public; license is Apache 2.0 (from GitHub's checkbox). Confirm both.
- Business model and distribution are undecided (see the report, section 11).

## How to run things

- Checks: `.\tools\check.ps1` (add `-Admin` in an elevated terminal for service tests).
- App: `cd apps\verity-ui; npm run tauri dev` (the service must be installed and running).
- UI preview in a browser with sample data: `npm --prefix apps/verity-ui run dev`, open `http://localhost:1420`.
- PresentMon is not in git: see `tools/presentmon/README.md`.

## Last session (2026-11-14 to 2026-11-17)

Built M0 and M1, the desktop app, the README and the report; verified M1 on
Forza; merged the app; started the FreeLLMAPI setup (waiting on the owner).
