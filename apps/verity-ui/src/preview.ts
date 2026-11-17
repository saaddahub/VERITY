// Sample data for previewing the UI in a browser, outside Tauri. The numbers are
// from real test recordings of the framegen tool on the development laptop.
// Never used inside the app.

import type { Game, Session, SessionStatus, StoryData, SystemInfo, Trace } from "./api.ts";

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

// The three games the library scan found on the development laptop, plus framegen.
const games: Game[] = [
  { id: 3, name: "BOMBANANA! Demo", exe_path: "C:\\Program Files (x86)\\Steam\\steamapps\\common\\BOMBANANA! Demo\\BOMBANANA.exe", source: "steam", launcher_id: "4747510" },
  { id: 4, name: "framegen", exe_path: "C:\\dev\\verity\\target\\debug\\framegen.exe", source: "manual", launcher_id: null },
  { id: 1, name: "Forza Horizon 5", exe_path: "C:\\Program Files (x86)\\Steam\\steamapps\\common\\ForzaHorizon5\\ForzaHorizon5.exe", source: "steam", launcher_id: "1551360" },
  { id: 2, name: "RV There Yet?", exe_path: "C:\\Program Files (x86)\\Steam\\steamapps\\common\\Ride\\Ride\\Binaries\\Win64\\Ride-Win64-Shipping.exe", source: "steam", launcher_id: "3949040" },
];

let enabled = true;
const status = (): SessionStatus => ({
  status: { state: "idle", enabled },
  notices: [],
  capabilities: [],
});

/** Session 4's planted test: each stutter fired as a stress tool started. */
function storyFor(id: number): StoryData {
  const groups = sessions.find((s) => s.id === id)?.summary.stats.hitch_groups ?? [];
  const planted = id === 4;
  const events = planted
    ? groups.flatMap((g, i) => [
        { qpc: g.start_qpc - 400_000, duration_ticks: 0, kind: "proc_start", subject: "stress.exe", magnitude: 1 },
        { qpc: g.start_qpc - 300_000, duration_ticks: 2_500_000, kind: "cpu_burst", subject: "stress.exe", magnitude: 3.8 + (i % 3) * 0.1 },
      ])
    : [];
  return {
    session_id: id,
    events,
    events_truncated: false,
    qpc_freq: FREQ,
    story: {
      hitch_groups: groups.length,
      hitch_rate_per_min: planted ? 40.6 : 0,
      causes: planted
        ? [
            { kind: "proc_start", subject: "stress.exe", label: "likely", k: groups.length, n: groups.length, lift: 4.6, p: 1e-12, excess_share: 1, remedy: { type: "throttle", id: "background.process" } },
            { kind: "cpu_burst", subject: "stress.exe", label: "likely", k: groups.length, n: groups.length, lift: 3.4, p: 1e-9, excess_share: 1, remedy: { type: "throttle", id: "background.process" } },
          ]
        : [],
      unexplained: { in_game_cpu: 0, in_game_gpu: 0, unknown: 0 },
      capture_mode: "standard",
      caveats: planted ? ["cpu_burst and gpu_other timing is sampled at 4 Hz"] : [],
      groups: groups.map((g) => ({ start_qpc: g.start_qpc, end_qpc: g.end_qpc, worst_ms: g.worst_ms, excess_ms: g.worst_ms - 16.7, class: planted ? "cause" : "unknown" })),
      pairs_tested: planted ? 6 : 0,
    },
  };
}

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
    games_list: games,
    games_scan: games,
    story_export: `C:\\Users\\you\\Downloads\\verity-frame-story-${id}.html`,
  };
  if (cmd === "story_get") return storyFor(id) as T;
  if (cmd === "session_enable") enabled = Boolean(args?.on);
  if (cmd === "session_panic") enabled = false;
  if (["session_status", "session_enable", "session_stop", "session_panic"].includes(cmd)) return status() as T;
  if (["record", "service_setup", "games_add", "games_remove", "games_answer"].includes(cmd))
    throw new Error("Not available in the design preview.");
  return answers[cmd] as T;
}
