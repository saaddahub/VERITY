# Verity — instructions for Claude Code

Verity is a Windows performance tool, for games and everyday use, that **measures first, changes second, and proves every change**. This file is the standing brief. The full spec is in `docs/`.

## Read before coding

0. `MEMORY.md` — read it first: where the project stands, the owner's preferences, and what is next. Update it at the end of every session.
1. `docs/01-vision.md` — what we are building and why it is different
2. `docs/02-architecture.md` — processes, crates, IPC, storage
3. `docs/09-safety.md` — binding rules; violations are bugs
4. `docs/11-roadmap.md` — milestones and acceptance criteria
5. The subsystem spec for the milestone in progress (`03`–`08`, `10`)

## Prime directives

1. **Measure, do not guess.** No optimization ships without a way to measure its effect on this machine.
2. **Everything is reversible.** Every system change goes through the Action framework (`docs/05-actions.md`). The snapshot of the prior state is written to the journal **before** the change is made.
3. **Hands off the game.** Never read or write a game's memory, never inject a DLL, never hook, never install a kernel driver, never modify game files. See `docs/09-safety.md`.
4. **No snake oil.** The blacklist in `docs/09-safety.md` is binding. Do not add a tweak because "optimizers usually do it".
5. **Honest output.** If Verity cannot show an effect, it says "no measurable effect". No fake scores, no invented percentages.
6. **One milestone at a time.** Do not start milestone N+1 until every acceptance criterion of N passes.

## Stack

- Rust stable, edition 2021, Cargo workspace, target `x86_64-pc-windows-msvc`
- Supported OS: Windows 10 22H2 and Windows 11 (all current builds)
- `windows` crate (windows-rs) for Win32, with only the features in use enabled
- `ferrisetw` for ETW consumption (fall back to raw `StartTraceW`/`ProcessTrace` in `verity-sys` if it cannot do something)
- `windows-service` for the service host
- `tokio` runtime, named pipes for IPC
- `rusqlite` with the `bundled` feature, `zstd` for frame chunks
- `serde`, `serde_json`, `thiserror` in libraries, `anyhow` in binaries only, `tracing`
- UI: Tauri 2, React, TypeScript, Vite, uPlot for charts
- Statistics: implemented in `verity-lab` with tests. Randomness is `rand_chacha`, always seeded and the seed stored.

## Commands

```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace -- --ignored        # admin-only integration tests, elevated terminal
cd apps/verity-ui && npm install && npm run tauri dev
```

A change is not done until build, test, clippy and fmt all pass.

## Code rules

- All `unsafe` and all raw Win32 calls live in the `verity-sys` crate behind safe wrappers. Every `unsafe` block has a `// SAFETY:` comment.
- Every Win32 call checks its result. Errors carry the API name and the Win32 error code or HRESULT.
- No `unwrap`, `expect` or `panic!` in library crates outside tests.
- Logic talks to the OS through traits (`ProcessApi`, `PowerApi`, `RegistryApi`, `DisplayApi`, …) so it is unit-testable with fakes. Real implementations live in `verity-sys`.
- Every Action supports dry-run and prints the exact change it would make.
- Process identity is always `(pid, process creation time)`, never a bare pid.
- PDH counters are added with `PdhAddEnglishCounterW` so non-English Windows works.
- All timestamps stored are QPC ticks; convert at the display edge only.
- Undocumented APIs go in `verity-sys::undocumented`, behind a cargo feature, with a runtime capability check and a documented fallback. Record the Windows builds on which each was verified.
- Logs go to `%ProgramData%\Verity\logs` with rotation. Never log file contents, window titles or anything personal beyond process names and paths.
- Do not add a dependency without stating the reason in the commit message.

## Security rules for the service

The service runs as SYSTEM and takes requests from user-level processes, so it is a privilege boundary.

- The service only executes actions from the compiled-in catalog, with parameters validated against each action's schema.
- The service never runs a command line, loads a DLL, or writes to a path supplied by a client.
- Registry paths, service names and power GUIDs are constants or come from an allowlist, never from client input.
- The pipe has an explicit DACL. See `docs/02-architecture.md`.

## When the spec and reality disagree

Windows details in these specs were written from knowledge, not verified on this machine. Items marked **(verify)** must be checked before being relied on.

- Write a small probe binary in `tools/probes/` and test the behavior instead of guessing.
- If the spec is wrong, make the code match reality, update the spec, and add an entry to `docs/notes/decisions.md` saying what changed and why.

## Ask the human before

- Anything that needs a reboot
- Anything that touches boot configuration (BCD)
- Any HKLM write that is not already listed in `docs/05-actions.md`
- Adding an action that is not in the catalog
- Loosening any rule in `docs/09-safety.md`
