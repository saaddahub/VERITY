// Sample data for previewing the UI in a browser, outside Tauri. The numbers are
// from real test recordings of the framegen tool on the development laptop.
// Never used inside the app.

import type { Session, SystemInfo, Trace } from "./api.ts";

const FREQ = 10_000_000;

function makeTrace(seconds: number, hitchAt: number[], seed: number): Trace {
  const qpc: number[] = [];
  const ms: number[] = [];
  let t = 1_580_000_000_000 + seed;
  let next = 0;
  for (let i = 0; i < seconds * 60; i++) {
    const s = i / 60;
    let f = 16.667 + Math.sin(i * 1.7 + seed) * 0.05 + Math.cos(i * 0.31) * 0.04;
    if (next < hitchAt.length && s >= hitchAt[next]) {
      f = 66.7 + Math.sin(i) * 0.2;
      next++;
    }
    qpc.push(t);
    ms.push(f);
    t += Math.round((f / 1000) * FREQ);
  }
  return { qpc, ms };
}

const hitchSeconds = Array.from({ length: 20 }, (_, i) => 10 + i);
const traces: Record<number, Trace> = {
  4: makeTrace(35, hitchSeconds, 0),
  5: makeTrace(13, [], 7),
};

function groupsFor(trace: Trace) {
  return trace.ms.flatMap((m, i) =>
    m !== null && m > 40
      ? [{ start_qpc: trace.qpc[i], end_qpc: trace.qpc[i] + Math.round((m / 1000) * FREQ), hitches: 1, worst_ms: m }]
      : [],
  );
}

const sessions: Session[] = [
  {
    id: 5,
    started_utc: "2026-11-14T12:26:13Z",
    frame_chunks: 2,
    summary: {
      exe: "framegen.exe", pid: 15248, capture_s: 13.6, warmup_excluded_s: 5, total_frames: 767,
      stats: { frame_count: 472, duration_s: 7.87, avg_fps: 60.0, low_1pct_fps: 59.5, low_01pct_fps: 59.1, pacing_spread_ms: 0.08, hitch_groups: [], hitch_rate_per_min: 0 },
      gpu_adapter: "Intel(R) UHD Graphics", capture_gaps: [], overhead_cpu_pct: 0.181, overhead_budget_pct: 0.3,
    },
  },
  {
    id: 4,
    started_utc: "2026-11-14T12:25:35Z",
    frame_chunks: 4,
    summary: {
      exe: "framegen.exe", pid: 13672, capture_s: 35.7, warmup_excluded_s: 5, total_frames: 2026,
      stats: { frame_count: 1732, duration_s: 29.54, avg_fps: 58.63, low_1pct_fps: 15.0, low_01pct_fps: 14.97, pacing_spread_ms: 0.08, hitch_groups: groupsFor(traces[4]), hitch_rate_per_min: 40.6 },
      gpu_adapter: "Intel(R) UHD Graphics", capture_gaps: [], overhead_cpu_pct: 0.179, overhead_budget_pct: 0.3,
    },
  },
];

const system: SystemInfo = {
  windows: { major: 10, build: 26200, ubr: 9974, display_version: "25H2" },
  cpu: { model: "11th Gen Intel(R) Core(TM) i5-11400H @ 2.70GHz", logical: Array.from({ length: 12 }, () => ({ kind: "uniform" as const })) },
  gpus: [
    { name: "NVIDIA GeForce RTX 3050 Laptop GPU", vendor: "NVIDIA", vendor_id: 0x10de, dedicated_vram_bytes: 4_179_623_936 },
    { name: "Intel(R) UHD Graphics", vendor: "Intel", vendor_id: 0x8086, dedicated_vram_bytes: 134_217_728 },
  ],
  displays: [{ device: "\\\\.\\DISPLAY1", width: 1920, height: 1080, current_hz: 144, max_hz: 144 }],
  power: { ac_online: true, battery_present: true, charging: false, battery_percent: 80 },
  power_scheme: { guid: "6fecc5ae-f350-48a5-b669-b472cb895ccf", name: "Turbo" },
};

export async function preview<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((r) => setTimeout(r, 120));
  const id = Number(args?.id);
  const answers: Record<string, unknown> = {
    service_status: { state: "running" },
    system_info: system,
    sessions_list: sessions,
    session_get: sessions.find((s) => s.id === id),
    session_trace: traces[id],
    gpu_apps: [
      { pid: 21260, name: "framegen.exe", gpu_pct: 35.1 },
      { pid: 9804, name: "chrome.exe", gpu_pct: 4.2 },
    ],
    export_csv: `C:\\Users\\you\\Downloads\\verity-session-${id}.csv`,
  };
  if (cmd === "record" || cmd === "service_setup") throw new Error("Not available in the design preview.");
  return answers[cmd] as T;
}
