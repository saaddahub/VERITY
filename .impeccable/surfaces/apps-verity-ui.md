---
version: 1
slug: "apps-verity-ui"
primary_target: "apps/verity-ui"
related_targets: []
---

# Surface brief: Verity desktop app (v0)

Scope: the Tauri desktop app shell with three screens, Checkup (home), Sessions, Record. Visitor mode: Operate.

Audience and job: gamers and everyday PC users equally, mixed technical level. Job: see the plain answer about the last recording and this PC, then record again or open the proof. Both a full window and a compact window (narrow) must work.

Constraints: no AI slop, no gamer RGB, no sterile dashboard, no hacker terminal, no walls of text (user, 2026-11-15). Unbuilt features (Doctor, actions, Frame Story, Lab) stay invisible. Answer first, numbers one click away.

Decision: the user declined to choose on the decision page ("forget the choosing"), so the rolled direction stands.

## Direction contract

THESIS: Verity reads the PC like a garage inspection sheet: each system measured against its limit and marked OK, worth a look, or fix it, under a plain verdict sentence. It refuses the stat-tile dashboard and the instrument costume.

OWN-WORLD: graphite ground with a darker rail, ruled sheet rows, and three-position check marks whose green, amber and red are the only state colours; one spot blue for actions and selection. Workshop-form grotesk (Barlow) for text, its condensed cut for column heads, a mono only for readings.

STORY: the user reads one sentence that says how the last recording went, scans the sheet to see what is fine and what is not, then records again or opens the session for the evidence.

FIRST VIEWPORT: left rail (Checkup, Sessions, Record; service status at the foot). Top of the work area: the verdict sentence as the page heading with the app, time and length beneath it, actions at right (Open session, Record again as primary). Below: four readings in one row. Then the inspection sheet: System, Reading, Limit, three-position mark, Note.

FORM: Workshop Inspection Report, position 4 of the ordered list, seed key a5e9c067. Signature interaction: when a sheet loads, each row's mark ticks into its position in sequence; readings update in place with tabular numerals so nothing shifts.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
