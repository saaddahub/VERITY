# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The app is a Tauri 2 desktop app on Windows 10 22H2 and Windows 11: a web UI in the system WebView2, with a tray icon. Design language is the app's own, not native Windows controls.

## Users

Two audiences, served equally from the first screen (confirmed 2026-11-15):

- **Gamers**, mostly on gaming laptops, who just played or are about to play and want to know whether a game runs smoothly and why it stutters.
- **Everyday PC users** whose machine feels slow, loud or drains its battery, and who want to know what is wrong.

Technical level is mixed: lead with a plain answer ("this game is rendering on the integrated GPU"), with the full numbers and evidence one click away.

## Product Purpose

Verity tells you exactly what is making your PC slow or unsmooth, in games and in everyday use, fixes it temporarily and reversibly, and proves the fix worked on your machine. Success is a user who understands what was wrong, sees the evidence, and keeps only changes that measurably helped.

## Positioning

A closed loop no neighbouring tool offers: measure, attribute (each stutter matched to its cause with a statistical lift), act (temporary, journaled, crash-safe changes), verify (randomized A/B experiments). It refuses snake-oil tweaks and says "no measurable effect" when that is the truth. Fully offline, passive toward games.

## Operating Context

- A background Windows service does the privileged work; the app is a user-session client over a named pipe, the same as the `verity` CLI.
- Typical moments: before or right after a gaming session; when the PC "feels slow"; reviewing past sessions.
- Laptop panels down to 1366×768 must be fully usable.

## Capabilities and Constraints

- Working today: service status, system info (Windows build, CPU and core classes, GPUs, displays with refresh, power), recording any app that draws with the GPU (average FPS, 1% and 0.1% lows, pacing, hitches, GPU used, Verity's overhead), session list, session detail, CSV export.
- Planned, not yet built: Doctor findings, reversible actions and the journal, Session Mode, Frame Story, the Lab, desktop smoothness capture. The UI must not show or imply these as working.
- Recording needs the service; installing the service needs one Windows admin prompt.
- Never stored or shown: window titles, command lines, personal data.
- Terminology: session, hitch (stutter), 1% low, frame time, capture.

## Brand Commitments

- Name: Verity.
- From `docs/10-ui.md`: "an instrument, not a billboard"; no health score, no "BOOST" button, no fake gauges; every number has a unit; plain, specific language ("Chrome used 14% CPU during your match").
- Honest output: "no measurable effect" is a real result shown with the same prominence as an improvement.

## Evidence on Hand

- Real recorded sessions from the development laptop in `C:\ProgramData\Verity\verity.db` (framegen test runs; no real game recorded yet).
- No logo, testimonials, customers or benchmarks exist. Do not invent them.

## Product Principles

1. Evidence beats folklore: every claim links to its numbers.
2. Plain answer first, proof one click away.
3. Temporary beats permanent; the user is in charge.
4. Say plainly when the bottleneck is the game or the hardware.
5. Never pretend: unbuilt features stay invisible, and "no effect" is shown as honestly as a win.

## Accessibility & Inclusion

WCAG 2.2 AA contrast, full keyboard navigation with visible focus, color never the only signal, reduced-motion support, usable at 1366×768, no layout shift when live numbers update (from `docs/10-ui.md`).
