// The app's only door to the service. Inside Tauri it calls the Rust commands;
// in a plain browser (design preview only) it serves recorded sample data.

export type Mark = "ok" | "look" | "fix" | "none";

export interface Gpu {
  name: string;
  vendor: string;
  vendor_id: number;
  dedicated_vram_bytes: number;
}
export interface Display {
  device: string;
  width: number;
  height: number;
  current_hz: number;
  max_hz: number;
}
export interface SystemInfo {
  windows: { major: number; build: number; ubr: number; display_version: string | null };
  cpu: { model: string; logical: { kind: "performance" | "efficiency" | "uniform" }[] };
  gpus: Gpu[];
  displays: Display[];
  power: { ac_online: boolean | null; battery_present: boolean; charging: boolean; battery_percent: number | null };
  power_scheme: { guid: string; name: string | null };
}
export interface HitchGroup {
  start_qpc: number;
  end_qpc: number;
  hitches: number;
  worst_ms: number;
}
export interface Summary {
  exe: string;
  pid: number;
  capture_s: number;
  warmup_excluded_s: number;
  total_frames: number;
  stats: {
    frame_count: number;
    duration_s: number;
    avg_fps: number;
    low_1pct_fps: number;
    low_01pct_fps: number;
    pacing_spread_ms: number;
    hitch_groups: HitchGroup[];
    hitch_rate_per_min: number;
  };
  gpu_adapter: string | null;
  capture_gaps: { qpc: number; duration_ticks: number }[];
  overhead_cpu_pct: number;
  overhead_budget_pct: number;
}
/** Stutter count and worst frame, carried by list rows and full sessions alike. */
export interface Hitches {
  count: number;
  worst_ms: number;
}
/** A `sessions_list` row: the summary without the lists that grow with the session. */
export interface SessionRow {
  id: number;
  started_utc: string;
  frame_chunks: number;
  hitches: Hitches;
  summary: Omit<Summary, "stats" | "capture_gaps"> & { stats: Omit<Summary["stats"], "hitch_groups"> };
}
/** One session in full (`session_get`). */
export interface Session extends SessionRow {
  summary: Summary;
}
export interface Trace {
  qpc: number[];
  ms: (number | null)[];
}
export interface GpuApp {
  pid: number;
  name: string;
  gpu_pct: number;
}
export interface GameRef {
  id: number | null;
  name: string;
  exe: string;
}
export type SessionState =
  | { state: "idle"; enabled: boolean }
  | { state: "arming"; enabled: boolean; game: GameRef; proc: { pid: number }; frames?: number }
  | { state: "active"; enabled: boolean; game: GameRef; proc: { pid: number }; frames?: number; exited_at: number | null };
export interface Notice {
  id: number;
  kind: "info" | "session_end" | "ask_game";
  text: string;
  session_id?: number;
  exe?: string;
}
export interface SessionStatus {
  status: SessionState;
  notices: Notice[];
  capabilities: string[];
}
export interface Game {
  id: number;
  name: string;
  exe_path: string;
  source: "steam" | "epic" | "gog" | "xbox" | "manual";
  launcher_id: string | null;
}
export interface Cause {
  kind: string;
  subject: string;
  label: "likely" | "possible";
  k: number;
  n: number;
  lift: number;
  p: number;
  excess_share: number;
  remedy: { type: string; id: string };
}
export interface StoryEvent {
  qpc: number;
  duration_ticks: number;
  kind: string;
  subject: string;
  magnitude: number;
}
export interface Story {
  hitch_groups: number;
  hitch_rate_per_min: number;
  causes: Cause[];
  unexplained: { in_game_cpu: number; in_game_gpu: number; unknown: number };
  capture_mode: string;
  caveats: string[];
  groups: { start_qpc: number; end_qpc: number; worst_ms: number; excess_ms: number; class: string }[];
  pairs_tested: number;
}
export interface StoryData {
  session_id: number;
  story: Story;
  events: StoryEvent[];
  events_truncated: boolean;
  qpc_freq: number;
}
/* ---------- Storage (docs/14-storage.md) ---------- */
export interface Drive {
  root: string;
  total: number;
  free: number;
}
export interface ScanSummary {
  root: string;
  counted: number;
  files: number;
  unreadable_folders: number;
  drive_total: number;
  drive_used: number;
  not_visible: number;
  seconds: number;
  cancelled: boolean;
}
export interface ScanProgress {
  dirs: number;
  files: number;
  bytes: number;
  done: boolean;
}
export interface StorageItem {
  id: number;
  name: string;
  path: string;
  is_dir: boolean;
  bytes: number;
  files: number;
  /** Newest change inside, FILETIME. */
  newest: number;
  unreadable: boolean;
  online_only: boolean;
  link: boolean;
  deletable: boolean;
}
export interface Folder {
  id: number;
  path: string;
  bytes: number;
  files: number;
  crumbs: { id: number; name: string }[];
  items: StorageItem[];
  more: { count: number; bytes: number };
}
export interface TypeGroup {
  group: string;
  bytes: number;
  files: number;
}
export interface DupGroup {
  size: number;
  wasted: number;
  items: StorageItem[];
}
export interface Suggestion {
  id: string;
  title: string;
  detail: string;
  kind: "delete" | "empty_recycle_bin" | "open_storage_sense";
  bytes: number | null;
  items: { path: string; bytes: number; modified: number; is_dir: boolean }[];
  preselected: boolean;
}
export interface DeleteResult {
  deleted: string[];
  kept: { path: string; reason: string }[];
  freed: number;
}
export interface Disk {
  model: string;
  media: "SSD" | "HDD" | "SCM" | "Unknown";
  health: "Healthy" | "Warning" | "Unhealthy" | "Unknown";
  size: number;
  wear: number | null;
  temperature: number | null;
  temperature_max: number | null;
  read_errors: number | null;
  write_errors: number | null;
  power_on_hours: number | null;
}
export interface Health {
  disks: Disk[];
  volumes: { letter: string; file_system: string; size: number; free: number }[];
}

export type ServiceState =
  | { state: "running" }
  | { state: "stopped" }
  | { state: "missing" }
  | { state: "error"; message: string };

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  const { preview } = await import("./preview.ts");
  return preview<T>(cmd, args);
}

export const api = {
  isPreview: !inTauri,
  serviceStatus: () => invoke<ServiceState>("service_status"),
  serviceSetup: (install: boolean) => invoke<void>("service_setup", { install }),
  systemInfo: () => invoke<SystemInfo>("system_info"),
  gpuApps: () => invoke<GpuApp[]>("gpu_apps"),
  record: (pid: number, durationS: number | null) =>
    invoke<{ id: number }>("record", { pid, durationS }),
  sessions: () => invoke<SessionRow[]>("sessions_list"),
  session: (id: number) => invoke<Session>("session_get", { id }),
  trace: (id: number, chunks: number) => invoke<Trace>("session_trace", { id, chunks }),
  exportCsv: (id: number, chunks: number) => invoke<string>("export_csv", { id, chunks }),
  sessionStatus: (since: number) => invoke<SessionStatus>("session_status", { since }),
  sessionEnable: (on: boolean) => invoke<SessionStatus>("session_enable", { on }),
  sessionStop: () => invoke<SessionStatus>("session_stop"),
  sessionPanic: () => invoke<SessionStatus>("session_panic"),
  games: () => invoke<Game[]>("games_list"),
  gamesScan: () => invoke<Game[]>("games_scan"),
  gamesAdd: (exePath: string) => invoke<{ id: number }>("games_add", { exePath }),
  gamesRemove: (id: number) => invoke<unknown>("games_remove", { id }),
  gamesAnswer: (exe: string, answer: "game" | "not_game" | "never") =>
    invoke<unknown>("games_answer", { exe, answer }),
  story: (id: number) => invoke<StoryData>("story_get", { id }),
  storyExport: (id: number) => invoke<string>("story_export", { id }),
};

/** Listens to a Tauri event; in the browser preview nothing is ever sent. */
async function listen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(event, (e) => cb(e.payload));
}

/** Storage runs in the app under the user's account; only health asks the service. */
export const storage = {
  drives: () => invoke<Drive[]>("storage_drives"),
  scan: (root: string) => invoke<ScanSummary>("storage_scan", { root }),
  cancel: () => invoke<void>("storage_cancel"),
  children: (id: number) => invoke<Folder>("storage_children", { id }),
  largest: () => invoke<StorageItem[]>("storage_largest"),
  types: () => invoke<TypeGroup[]>("storage_types"),
  duplicates: () => invoke<DupGroup[]>("storage_duplicates"),
  suggestions: () => invoke<Suggestion[]>("storage_suggestions"),
  remove: (paths: string[]) => invoke<DeleteResult>("storage_delete", { paths }),
  emptyRecycleBin: () => invoke<void>("storage_empty_recycle_bin"),
  reveal: (path: string) => invoke<void>("storage_reveal", { path }),
  openStorageSense: () => invoke<void>("storage_open_storage_sense"),
  health: () => invoke<Health>("storage_health"),
  optimize: (drive: string) => invoke<{ drive: string; result: number }>("storage_optimize", { drive }),
  onProgress: (cb: (p: ScanProgress) => void) => listen<ScanProgress>("storage-progress", cb),
  onDupesProgress: (cb: (p: { done: number; total: number }) => void) => listen("storage-dupes-progress", cb),
};
