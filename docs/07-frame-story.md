# 07 — Frame Story (stutter forensics)

The feature that makes Verity different. For every hitch in a session, Frame Story answers: *what was the system doing at that moment, and is that a coincidence?*

## Pipeline

```
frames ──▶ hitch detection ──▶ hitch groups ─┐
                                             ├─▶ coverage ──▶ lift + significance ──▶ ranked causes ──▶ remedies
events (DPC, faults, I/O, scans, …) ─────────┘
```

Runs at session end, and on demand for any stored session.

## Step 1 — hitches

Use the definitions in `03-telemetry.md` (hitch, hitch group). For each group store: start QPC, end QPC, worst frame time, total excess time (sum of frame time above the rolling median).

Frames inside capture gaps, the first 5 seconds, and non-foreground periods are excluded.

## Step 2 — candidate events

Each event kind has a subject (what to blame) and an influence window (how long after it starts it could plausibly affect a frame).

| Kind | Subject | Counts as an event when | Influence window |
|---|---|---|---|
| `dpc` / `isr` | driver file | a single routine > 500 µs, or a 10 ms bucket where the driver's total > 2 ms | event duration + 20 ms |
| `hard_fault` | faulting process (game or other) | ≥ 5 hard faults in 100 ms | 100 ms |
| `disk_io` | process | any I/O taking > 50 ms, or > 20 MB in 250 ms by a non-game process | event duration + 50 ms |
| `cpu_burst` | process | a non-game process using > 1 core-equivalent in a 250 ms window | the window |
| `proc_start` | image name | any process start | 1 s |
| `throttle` | "CPU power/thermal limit" | performance limit drops by ≥ 10 points | until it recovers, max 10 s |
| `power_source` | "AC ↔ battery" | any change | 3 s |
| `scan` | "Defender scan" | scan running | scan duration |
| `update` | "Windows Update" | install/download running | its duration |
| `task` | task name | scheduled task started | 5 s |
| `gpu_other` | process | a non-game process using > 20% of the game's GPU engine | the window |
| `foreground` | "focus change" | foreground changed | 500 ms |
| `ctx_contention` (Deep) | preempting process | game render thread ready-but-not-running > 2 ms | event duration |

`cpu_burst` and `gpu_other` come from 4 Hz samples in Standard mode, so their timing is coarse; say so in the UI.

## Step 3 — coverage

A hitch group is **covered** by an event if the event's influence window overlaps the interval `[group start − worst frame time, group end]`. In words: the event was happening while the slow frame was being produced.

## Step 4 — is it a coincidence?

For each `(kind, subject)` pair:

- `N` = number of hitch groups in the session
- `k` = number of hitch groups covered by this pair
- `p0` = fraction of eligible session time that falls inside this pair's influence windows (merged, so overlaps are not double-counted), widened by the average hitch duration
- expected = `N × p0`
- **lift** = `k / expected`
- **p-value** = probability of seeing `k` or more out of `N` under a binomial with rate `p0`

Classification:

| Label | Rule |
|---|---|
| **Likely cause** | `k ≥ 3`, lift ≥ 3, p < 0.01 |
| **Possible cause** | `k ≥ 2`, lift ≥ 2, p < 0.05 |
| Not reported | everything else |

When many pairs are tested, apply the Holm correction across the pairs of one session before labeling. Always show `k`, `N`, lift, and the share of total excess time covered. Use the words "coincides with", never "causes".

### Why this matters

An event that happens all the time (large `p0`) will overlap many hitches by chance; lift corrects for that. An event that is rare but is present at most hitches stands out even with few occurrences.

## Step 5 — unexplained hitches

A hitch group with no likely or possible cause is classified by what the frame itself shows:

| Class | Signal | What Verity says |
|---|---|---|
| In-game, CPU side | `cpu_busy` spikes, no external events | "Originates inside the game (commonly shader compilation or asset streaming). System tweaks will not remove it." |
| In-game, GPU side | `gpu_busy` spikes, no external events | "The GPU took longer on these frames. Likely scene load; consider lowering GPU-heavy settings." |
| Unknown | neither | "No correlated system event found at this capture level." Offer a Deep capture next time. |

Saying "this is the game, not your PC" is a feature. It stops users from chasing tweaks that cannot help.

## Step 6 — across sessions

Pool the counts (`k`, `N`, exposure time) per `(game, kind, subject)` and per `(machine, kind, subject)` over the last 20 sessions and compute the same statistics. Pooled evidence can promote a "possible" to "likely". The UI states the sample: "coincides with 41% of hitches across 12 sessions".

## Step 7 — from cause to fix

| Cause | Offered remedy |
|---|---|
| Background process CPU or I/O | Add to the session's throttling set; show whether it was excluded and why; guide to pause/close it |
| Driver DPC/ISR | Guide by device class, naming the driver file |
| Defender scan, Windows Update, scheduled task | Scheduling guidance |
| Throttle events | Cooling and vendor-profile guidance; link to Doctor D013 |
| Hard faults in the game | Memory-pressure guidance; link to D021 |
| Overlay module present plus contention | Suggest a Lab experiment with the overlay off |

After a remedy is applied, later sessions recompute that cause's coverage. If it drops below the "possible" threshold over at least 3 sessions, mark **verified fixed** and show before/after hitch rates.

## Output

### Data (`story.get`)

```json
{
  "session_id": 42,
  "hitch_groups": 37,
  "hitch_rate_per_min": 1.8,
  "causes": [
    { "kind": "dpc", "subject": "example.sys", "label": "likely",
      "k": 14, "n": 37, "lift": 9.2, "p": 0.00002, "excess_share": 0.44,
      "remedy": { "type": "guide", "id": "driver.network" } }
  ],
  "unexplained": { "in_game_cpu": 9, "in_game_gpu": 4, "unknown": 3 },
  "capture_mode": "standard",
  "caveats": ["cpu_burst timing is sampled at 4 Hz"]
}
```

### Timeline view

A frame-time graph on top, with event lanes beneath it sharing the same time axis, one lane per kind. Hitch groups are marked; selecting one lists the events covering it, sorted by magnitude. Spec in `10-ui.md`.

### Shareable report

`verity story export <session> --html <path>`: a single self-contained HTML file with the summary, causes and a static timeline. Process names and driver names only; no paths, no user name.

## Tests

- Unit tests on synthetic sessions: planted correlation is found; uncorrelated frequent events are not reported; the binomial and Holm code is checked against hand-computed values.
- End-to-end with `tools/framegen` and `tools/stressors` (see `11-roadmap.md`): a planted cause is named as likely; a decoy process that starts on a fixed schedule unrelated to the hitches is not reported.
