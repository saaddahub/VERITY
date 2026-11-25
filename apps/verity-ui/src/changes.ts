// Plain-language view of Verity's change journal (docs/05-actions.md).
import type { ActionInfo, JournalRow } from "./api.ts";

export type Phase = "on" | "back" | "kept" | "failed";

/** What a row's state means to the user. */
export function phase(state: JournalRow["state"]): Phase {
  if (state === "pending" || state === "applied") return "on";
  if (state === "superseded") return "kept";
  if (state === "revert_failed") return "failed";
  return "back";
}

export const phaseLabel: Record<Phase, string> = {
  on: "In effect",
  back: "Put back",
  kept: "Your change kept",
  failed: "Could not put back",
};

/** One user-level event: a game session, or one change made by hand. */
export interface Group {
  txn: string;
  source: string;
  started: string;
  rows: JournalRow[];
  /** e.g. "Efficiency mode for 118 background apps · Best performance power mode" */
  summary: string;
  open: number;
  phase: Phase;
}

const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/** The specific thing a row changed, for one-row groups and details. */
export function detail(r: JournalRow): string {
  const p = r.params as Record<string, unknown>;
  switch (r.action_id) {
    case "gpu.preference":
      return fileName(String(p.exe ?? ""));
    case "display.max_refresh":
      return `${p.hz} Hz`;
    case "startup.disable_item":
      return String(p.name ?? "");
    case "sched.background_ecoqos":
    case "sched.background_cpusets":
      return String(p.name ?? "");
    default:
      return "";
  }
}

function describe(rows: JournalRow[], catalog: Map<string, ActionInfo>): string {
  const counts = new Map<string, JournalRow[]>();
  for (const r of rows) counts.set(r.action_id, [...(counts.get(r.action_id) ?? []), r]);
  return [...counts.entries()]
    .map(([id, rs]) => {
      const title = catalog.get(id)?.title ?? id;
      if (id === "sched.background_ecoqos") return `Efficiency mode for ${rs.length} background ${rs.length === 1 ? "app" : "apps"}`;
      if (id === "sched.background_cpusets") return `${rs.length} background ${rs.length === 1 ? "app" : "apps"} on efficiency cores`;
      if (id === "power.session_overlay") return "Best performance power mode";
      const what = rs.map(detail).filter(Boolean).join(", ");
      return what ? `${title}: ${what}` : title;
    })
    .join(" · ");
}

function sourceOf(txn: string): string {
  if (txn.startsWith("session-")) return "Game session";
  if (txn.startsWith("manual-")) return "From the command line";
  return "Made in Verity";
}

/** Groups rows by transaction, newest first. */
export function group(rows: JournalRow[], catalog: ActionInfo[]): Group[] {
  const byId = new Map(catalog.map((a) => [a.id, a]));
  const groups = new Map<string, JournalRow[]>();
  for (const r of rows) groups.set(r.txn_id, [...(groups.get(r.txn_id) ?? []), r]);
  return [...groups.entries()]
    .map(([txn, rs]) => {
      const phases = rs.map((r) => phase(r.state));
      const open = phases.filter((p) => p === "on").length;
      // "Kept" only when nothing was put back; a mix reads as put back, with a note.
      const worst: Phase = open
        ? "on"
        : phases.includes("failed")
          ? "failed"
          : phases.every((p) => p === "kept")
            ? "kept"
            : "back";
      return {
        txn,
        source: sourceOf(txn),
        started: rs[0].created_utc,
        rows: rs,
        summary: describe(rs, byId),
        open,
        phase: worst,
      };
    })
    .sort((a, b) => Math.max(...b.rows.map((r) => r.id)) - Math.max(...a.rows.map((r) => r.id)));
}

/** A plan line for people: no device prefix, a real arrow. */
export function friendly(change: string): string {
  return change.replace(/^\\\\\.\\DISPLAY\d+:\s*/, "").replace(/ -> /g, " → ");
}

/** A startup item's name without its shortcut extension. */
export const itemName = (name: string) => name.replace(/\.lnk$/i, "");

/** "Today 14:05", "Yesterday 09:12" or "21 Nov 18:30", local time. */
export function when(utc: string, now = new Date()): string {
  const d = new Date(utc);
  if (Number.isNaN(d.getTime())) return utc;
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(now) - day(d)) / 86_400_000);
  if (diff === 0) return `Today ${time}`;
  if (diff === 1) return `Yesterday ${time}`;
  return `${d.toLocaleDateString(undefined, { day: "numeric", month: "short" })} ${time}`;
}
