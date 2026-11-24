// Sample data for previewing the UI in a browser, outside Tauri. The numbers are
// from real test recordings of the framegen tool on the development laptop.
// Never used inside the app.

import type {
  Folder,
  Game,
  JunkCategory,
  Health,
  ScanSummary,
  Session,
  SessionRow,
  SessionStatus,
  StorageItem,
  StoryData,
  Suggestion,
  SystemInfo,
  Trace,
} from "./api.ts";

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

const sessions: Session[] = ([
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
] satisfies Omit<Session, "hitches">[]).map((s) => ({
  ...s,
  hitches: {
    count: s.summary.stats.hitch_groups.length,
    worst_ms: Math.max(0, ...s.summary.stats.hitch_groups.map((g) => g.worst_ms)),
  },
}));

/** What `sessions_list` sends: no per-stutter or per-gap lists. */
function row(s: Session): SessionRow {
  const { capture_gaps: _gaps, stats, ...summary } = s.summary;
  const { hitch_groups: _groups, ...listStats } = stats;
  return { ...s, summary: { ...summary, stats: listStats } };
}

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

/* Storage: the C: drive scan of the development laptop (tools/probes storage).
   Top-level folder sizes are real; file names below them are samples. */
const GB = 1024 ** 3;
const FT_DAY = 864_000_000_000;
const ftNow = (Date.now() + 11_644_473_600_000) * 10_000;
const item = (id: number, path: string, bytes: number, o: Partial<StorageItem> = {}): StorageItem => ({
  id,
  name: path.split("\\").pop() ?? path,
  path,
  is_dir: true,
  bytes,
  files: Math.round(bytes / 400_000),
  newest: ftNow - 2 * FT_DAY,
  unreadable: false,
  online_only: false,
  link: false,
  deletable: true,
  ...o,
});
const drive = { root: "C:\\", total: 484_569_321_472, free: 109_058_310_144 };
const scan: ScanSummary = {
  root: "C:\\",
  counted: 323_290_000_000,
  files: 1_335_970,
  unreadable_folders: 144,
  drive_total: drive.total,
  drive_used: drive.total - drive.free,
  not_visible: drive.total - drive.free - 323_290_000_000,
  seconds: 14.4,
  cancelled: false,
};
const top: StorageItem[] = [
  item(1, "C:\\Users", 120.79e9, { deletable: false }),
  item(2, "C:\\Riot Games", 36.13e9),
  item(3, "C:\\Windows", 34.6e9, { deletable: false }),
  item(4, "C:\\Medal", 25.95e9),
  item(5, "C:\\ProgramData", 20.1e9, { deletable: false }),
  item(6, "C:\\Program Files", 18.18e9, { deletable: false }),
  item(7, "C:\\dev", 18.16e9),
  item(8, "C:\\Program Files (x86)", 17.44e9, { deletable: false }),
  item(9, "C:\\System Volume Information", 0, { unreadable: true, deletable: false, files: 0 }),
];
const largest: StorageItem[] = [
  item(20, "C:\\Users\\you\\Downloads\\sample-installer.iso", 5.6 * GB, { is_dir: false, files: 1, newest: ftNow - 120 * FT_DAY }),
  item(21, "C:\\Medal\\Clips\\sample-clip-01.mp4", 3.9 * GB, { is_dir: false, files: 1, newest: ftNow - 40 * FT_DAY }),
  item(22, "C:\\Medal\\Clips\\sample-clip-02.mp4", 2.2 * GB, { is_dir: false, files: 1, newest: ftNow - 9 * FT_DAY }),
];
function folderFor(id: number): Folder {
  if (id === 0) {
    const shown = top.reduce((s, i) => s + i.bytes, 0);
    return {
      id: 0,
      path: "C:\\",
      bytes: scan.counted,
      files: scan.files,
      crumbs: [{ id: 0, name: "C:\\" }],
      items: top,
      more: { count: 31, bytes: scan.counted - shown },
    };
  }
  const f = top.find((i) => i.id === id) ?? top[0];
  return {
    id,
    path: f.path,
    bytes: f.bytes,
    files: f.files,
    crumbs: [{ id: 0, name: "C:\\" }, { id, name: f.name }],
    items: [],
    more: { count: 0, bytes: 0 },
  };
}
const suggestions: Suggestion[] = [
  {
    id: "recycle_bin",
    title: "Recycle Bin",
    detail: "Deleted items are still taking space. Emptying it cannot be undone.",
    kind: "empty_recycle_bin",
    bytes: 3.1 * GB,
    items: [],
  },
  {
    id: "installers",
    title: "Installers in Downloads",
    detail: "Setup files are usually not needed once the app is installed.",
    kind: "delete",
    bytes: 5.6 * GB,
    items: [{ path: "C:\\Users\\you\\Downloads\\sample-installer.iso", bytes: 5.6 * GB, modified: ftNow - 120 * FT_DAY, is_dir: false }],
  },
];
/* Junk sizes are the dev laptop's, from tools/probes cleanup (2026-11-24); browser is a sample. */
const MB = 1024 ** 2;
const junkSizes: Record<string, number> = {
  temp: 1.3 * MB, thumbnails: 164.2 * MB, browser: 412 * MB, internet: 4.4 * MB, shaders: 0.8 * MB,
  delivery: 16 * MB, error_reports: 0.8 * MB, memory_dumps: 0, minidumps: 0, setup_logs: 0,
  setup_temp: 0, upgrade_logs: 0.3 * MB, chkdsk: 0, updates: 190 * MB,
};
const junkCatalog: JunkCategory[] = [
  ["temp", "Temporary files", "Files apps left in your temp folder and not used for a week.", "app", true],
  ["thumbnails", "Thumbnail cache", "Picture previews File Explorer redraws when you next open a folder.", "app", true],
  ["browser", "Browser caches", "Saved copies of web pages in Chrome, Edge, Brave and Firefox. Close the browser first; open files are skipped. You stay signed in.", "app", true],
  ["internet", "Temporary internet files", "The older Windows web cache some apps still use.", "app", true],
  ["shaders", "DirectX shader cache", "Games rebuild these, so the first launch of each game can stutter for a while afterwards.", "app", false],
  ["delivery", "Delivery Optimization files", "Update pieces Windows keeps to share with other PCs.", "service", true],
  ["error_reports", "Error reports", "Crash reports already sent to, or never needed by, Microsoft.", "service", true],
  ["memory_dumps", "Crash memory dumps", "Large files Windows saved after a crash. Only useful to diagnose that crash.", "service", true],
  ["minidumps", "Crash minidumps", "Small crash records, only useful to diagnose an old crash.", "service", true],
  ["setup_logs", "Setup logs", "Logs left by Windows setup.", "service", true],
  ["setup_temp", "Temporary setup files", "Files installers left behind.", "service", true],
  ["upgrade_logs", "Upgrade logs", "Logs from past Windows upgrades.", "service", true],
  ["chkdsk", "Old disk-check fragments", "Pieces saved by old disk checks.", "service", true],
  ["updates", "Windows Update leftovers", "Old versions of updated Windows files. Afterwards, installed updates cannot be uninstalled. Checking can take a few minutes.", "service", false],
].map(([id, title, detail, runs, preselected]) => ({
  id: id as string, title: title as string, detail: detail as string,
  runs: runs as "app" | "service", preselected: preselected as boolean, slow: id === "updates",
}));
const health: Health = {
  disks: [
    {
      model: "NVMe Micron_2400_MTFDKBA512QFM",
      media: "SSD",
      health: "Healthy",
      size: 512_110_190_592,
      wear: null,
      temperature: null,
      temperature_max: null,
      read_errors: null,
      write_errors: null,
      power_on_hours: null,
    },
  ],
  volumes: [{ letter: "C", file_system: "NTFS", size: drive.total, free: drive.free }],
};

export async function preview<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((r) => setTimeout(r, 120));
  const id = Number(args?.id);
  const answers: Record<string, unknown> = {
    service_status: { state: "running" },
    system_info: system,
    sessions_list: sessions.map(row),
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
    storage_drives: [drive],
    storage_scan: scan,
    storage_largest: largest,
    storage_types: [
      { group: "Game data", bytes: 61.2e9, files: 18_402 },
      { group: "System and app data", bytes: 58.7e9, files: 612_330 },
      { group: "Videos", bytes: 44.9e9, files: 312 },
      { group: "Programs and installers", bytes: 31.0e9, files: 9_840 },
      { group: "Other", bytes: 127.5e9, files: 695_086 },
    ],
    storage_duplicates: [],
    storage_suggestions: suggestions,
    storage_health: health,
  };
  if (cmd === "storage_children") return folderFor(id) as T;
  if (cmd === "storage_junk_catalog") return junkCatalog as T;
  if (cmd === "storage_junk_measure")
    return (args?.ids as string[]).map((i) => ({ id: i, bytes: junkSizes[i] ?? 0 })) as T;
  if (["storage_delete", "storage_empty_recycle_bin", "storage_optimize", "storage_junk_clean"].includes(cmd))
    throw new Error("Not available in the design preview.");
  if (cmd === "story_get") return storyFor(id) as T;
  if (cmd === "session_enable") enabled = Boolean(args?.on);
  if (cmd === "session_panic") enabled = false;
  if (["session_status", "session_enable", "session_stop", "session_panic"].includes(cmd)) return status() as T;
  if (["record", "service_setup", "games_add", "games_remove", "games_answer"].includes(cmd))
    throw new Error("Not available in the design preview.");
  return answers[cmd] as T;
}
