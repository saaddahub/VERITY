# 08 — The Lab

The Lab answers one question with evidence: **does this change help this game on this machine?**

Game benchmarks are noisy. Run-to-run variation of a few percent is normal, and most tweaks claim effects of that size. Without a design that accounts for noise, any before/after comparison is storytelling.

## Core design

- **Unit of analysis is a run, not a frame.** Frames within a run are strongly correlated; treating them as independent samples produces false certainty.
- **Randomized blocks.** A block is one run with the change off (A) and one with it on (B), in random order. Blocking cancels slow drift such as heat build-up and background state.
- **Fixed plan.** The number of blocks is chosen before starting. No stopping early because the numbers look good.
- **Seeded.** The order is generated from a stored seed so the experiment is reproducible.

## Run

| Parameter | Default |
|---|---|
| Warm-up before measuring | 30 s (discarded) |
| Measured duration | 90 s |
| Sitting warm-up | The first run of a sitting is an unrecorded practice run |
| Capture mode | Standard |

Metrics per run, from `03-telemetry.md`: average FPS, 1% low FPS, 0.1% low FPS, hitch rate, pacing spread.

- **Primary metric:** 1% low FPS (configurable per experiment).
- **Guardrails:** average FPS and hitch rate.

### Run validity

A run is invalid, and is repeated, if any of these occur during the measured window:

- power source changed
- game lost foreground
- a Defender scan or Windows Update install was active
- throttled-time share differs from the block partner by more than 20 points
- capture gap, or duration off by more than 5%
- bottleneck class is "capped" for more than 50% of the run (a frame cap hides any effect; tell the user to uncap)

## Protocols

| Protocol | How runs happen | Evidence grade |
|---|---|---|
| **Benchmark** | The game has a repeatable built-in benchmark. The user (or a user-configured launch command) starts it for each run; Verity detects start and end. | Strong |
| **Guided manual** | The user replays the same short route or scene each run. Verity prompts through notifications and sound cues from the tray agent: "Run 3 of 12: start your route now." | Medium; depends on the user's consistency |
| **Observational** | No dedicated runs. Verity alternates the change across normal play sessions over days and compares. | Weak; labeled "observational", never used to auto-keep a change |
| **Long-form** (M8) | For changes that need a reboot: alternate A and B across reboots, several runs per boot. | Medium |

Between runs Verity applies or reverts the action through the normal Action framework, waits for a settle time (default 10 s), then prompts.

## Modes

| Mode | Blocks | Time (approx.) | Verdicts available |
|---|---|---|---|
| Quick | 3 | 15 min | "large effect likely" or "inconclusive" only. Labeled indicative. |
| Rigorous | 6 or more | 30 min+ | all four verdicts |
| Calibration (A/A) | 4 | 20 min | none; measures the noise floor for this game and scene |

Six is the minimum for the rigorous test: with fewer blocks the exact test below cannot reach p < 0.05.

## Analysis (rigorous)

For each block `j`, relative difference `d_j = (B_j − A_j) / A_j` for each metric.

1. **Point estimate:** mean of `d_j`.
2. **Confidence interval:** 95% t-interval on the paired differences, `df = n − 1`. Use a hard-coded table of critical values.
3. **p-value:** exact sign-flip permutation test on the paired differences. Enumerate all `2^n` sign assignments; p is the share with `|mean| ≥ |observed mean|`.
4. **Practical threshold `T`:** 3% by default. Effects smaller than this are not worth a system change.

### Verdict on the primary metric

| Verdict | Rule |
|---|---|
| **Improves** | p < 0.05, CI lower bound > 0, point estimate ≥ +T, and no guardrail has a significant regression beyond −2% |
| **Hurts** | p < 0.05, CI upper bound < 0, point estimate ≤ −T |
| **No measurable effect** | the whole CI lies inside (−T, +T) |
| **Inconclusive** | anything else. Report the CI and how many more blocks would likely settle it. |

"No measurable effect" is a real result and is shown with the same prominence as an improvement. It is how Verity debunks folklore on the user's own hardware.

### Quick mode rule

"Large effect likely" if all 3 blocks agree in sign and the mean `|d|` ≥ 2T and exceeds twice the calibrated noise (if a calibration exists). Otherwise inconclusive, with a suggestion to run rigorous.

### Noise floor and planning

A calibration (A/A) stores the between-run coefficient of variation of each metric for a `(game, scene)`. From it, estimate blocks needed for a rigorous test: `n ≈ (t × sd_d / T)²` with `sd_d ≈ √2 × CV`, rounded up, minimum 6. Show this before the user starts: "This scene is noisy; expect about 10 blocks."

### Several experiments

Each experiment tests one action with one parameter set. When the user queues several, show each result separately and apply the Holm correction to the set before labeling any as "improves".

## Keeping a change

- **Improves** → offer "Keep for this game". Accepting enables the action in the game's profile with `"proven_by": <experiment id>`.
- **Hurts** or **No measurable effect** → the action stays off; the result is remembered so Doctor does not suggest it again for this game.
- Nothing is kept automatically.

## Result card

- Verdict in words
- Primary metric: A mean, B mean, relative difference with CI, p-value
- Guardrails in the same format
- A small paired plot: one line per block from A to B
- Runs: valid, repeated, and why
- Protocol, grade, seed, date, Verity version

## State machine

`created → calibrating? → running(block i, arm x) → analyzing → done | aborted`

Every step is persisted, so an experiment survives a game crash, a service restart or the user stopping for the day. `lab.next_step` returns the next instruction; `lab.prompt` notifications drive the agent's cues. Aborting reverts the action and keeps completed runs.

## What may be tested

Any action with `lab_testable = true`, plus user-defined "manual" changes: the user names something Verity cannot toggle (an in-game setting, an overlay, a driver option) and flips it when prompted. Blacklisted items (`09-safety.md`) cannot be tested.

## CLI

```
verity lab new --game <id> --action <id> [--param k=v] --protocol benchmark|manual --mode quick|rigorous|calibration
verity lab next <experiment>
verity lab status <experiment>
verity lab result <experiment> [--json]
verity lab abort <experiment>
```

## Tests

- Statistics: permutation p-values and t-intervals checked against hand-computed cases for n = 3..8; determinism under a fixed seed.
- Simulation test: generate synthetic runs with a known noise level. With a true effect of 0, "improves" or "hurts" must appear in at most 5% of 2,000 simulated rigorous experiments. With a true effect of 3T, "improves" must appear in at least 80%.
- End-to-end with `tools/framegen`: an A/A experiment does not return improves or hurts; an experiment where arm B runs framegen with a 10% lower frame cost returns "improves".
