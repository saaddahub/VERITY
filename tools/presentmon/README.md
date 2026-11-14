# PresentMon (third party)

Verity uses the PresentMon console application as its frame source (M1).

- Version: 2.6.0 (pinned)
- File: `PresentMon-2.6.0-x64.exe`, not committed; download it here from
  https://github.com/GameTechDev/PresentMon/releases/download/v2.6.0/PresentMon-2.6.0-x64.exe
- SHA-256: `b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af`
- Signed by Intel Corporation. Licence: `LICENSE.txt` (MIT).

The version, hash, flags and CSV columns Verity relies on are in
`crates/verity-telemetry/src/presentmon.rs` and were verified by running it;
see `docs/notes/decisions.md`.
