import { test } from "node:test";
import assert from "node:assert/strict";
import { checks, verdict } from "./checks.ts";
import type { SessionRow, SystemInfo } from "./api.ts";

const session = (over: Partial<SessionRow["summary"]> = {}, groups = 0): SessionRow => ({
  id: 1,
  started_utc: "2026-11-14T12:00:00Z",
  frame_chunks: 1,
  hitches: { count: groups, worst_ms: groups ? 50 + groups - 1 : 0 },
  summary: {
    exe: "game.exe", pid: 1, capture_s: 30, warmup_excluded_s: 5, total_frames: 1800,
    stats: {
      frame_count: 1500, duration_s: 25, avg_fps: 60, low_1pct_fps: 58, low_01pct_fps: 57, pacing_spread_ms: 0.1,
      hitch_rate_per_min: groups * 2.4,
    },
    gpu_adapter: "Intel(R) UHD Graphics", overhead_cpu_pct: 0.1, overhead_budget_pct: 0.3,
    ...over,
  },
});

const info = (over: Partial<SystemInfo> = {}): SystemInfo => ({
  windows: { major: 10, build: 26200, ubr: 1, display_version: "25H2" },
  cpu: { model: "CPU", logical: [] },
  gpus: [
    { name: "NVIDIA GeForce RTX 3050 Laptop GPU", vendor: "NVIDIA", vendor_id: 0x10de, dedicated_vram_bytes: 4 * 2 ** 30 },
    { name: "Intel(R) UHD Graphics", vendor: "Intel", vendor_id: 0x8086, dedicated_vram_bytes: 128 * 2 ** 20 },
  ],
  displays: [{ device: "D1", width: 1920, height: 1080, current_hz: 60, max_hz: 144 }],
  power: { ac_online: false, battery_present: true, charging: false, battery_percent: 40 },
  power_scheme: { guid: "g", name: "Balanced" },
  ...over,
});

test("verdict: smooth, a few stutters, many stutters, too short", () => {
  assert.equal(verdict(session()).mark, "ok");
  assert.match(verdict(session()).headline, /^game ran smoothly at 60\.0 FPS\.$/);
  assert.equal(verdict(session({}, 1)).mark, "look");
  assert.match(verdict(session({}, 1)).headline, /stuttered 1 time\./);
  assert.equal(verdict(session({}, 20)).mark, "fix");
  assert.match(verdict(session({}, 20)).detail, /worst frame took 69 ms/);
  const short = session({ stats: { ...session().summary.stats, frame_count: 0 } });
  assert.equal(verdict(short).mark, "none");
});

test("checks flag the integrated GPU, a low refresh rate and battery power", () => {
  const byName = Object.fromEntries(checks(session(), info()).map((c) => [c.system, c]));
  assert.equal(byName.Graphics.mark, "look");
  assert.match(byName.Graphics.limit, /RTX 3050 available/);
  assert.equal(byName.Display.mark, "look");
  assert.equal(byName.Power.mark, "fix");
  assert.equal(byName["Verity itself"].mark, "ok");
});

test("checks pass a healthy setup and never fire without data", () => {
  const healthy = info({
    displays: [{ device: "D1", width: 1920, height: 1080, current_hz: 144, max_hz: 144 }],
    power: { ac_online: true, battery_present: false, charging: false, battery_percent: null },
  });
  const marks = checks(session({ gpu_adapter: "NVIDIA GeForce RTX 3050 Laptop GPU" }), healthy).map((c) => c.mark);
  assert.deepEqual(marks, ["ok", "ok", "ok", "ok"]);
  assert.deepEqual(checks(null, null), []);
  assert.equal(checks(session({ gpu_adapter: null }), healthy)[0].mark, "none");
});
