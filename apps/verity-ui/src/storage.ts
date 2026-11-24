// Pure helpers for the Storage screen, kept apart from the view so they can be tested.

import type { Check } from "./checks.ts";
import type { Disk, Drive, Health } from "./api.ts";

/** Sizes in the units Windows Explorer shows (1 GB = 1024³ bytes). */
export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

/** "C" from "C:\". */
export const letter = (root: string) => root.slice(0, 1).toUpperCase();

/** Lower-case path compare, Windows style. */
const norm = (p: string) => p.replace(/[\\/]+$/, "").toLowerCase();

/** Leaves out paths inside another chosen folder, so nothing is counted or deleted twice. */
export function pruneNested(paths: string[]): string[] {
  // One spelling per path ("C:\A" and "c:\a\" are the same folder).
  const unique = new Map<string, string>();
  for (const p of paths) if (!unique.has(norm(p))) unique.set(norm(p), p);
  const keys = [...unique.keys()];
  return [...unique].filter(([n]) => !keys.some((q) => q !== n && n.startsWith(`${q}\\`))).map(([, p]) => p);
}

/** Total of a selection, counting a folder once even if items inside it are also chosen. */
export function selectedBytes(sel: Map<string, number>): number {
  return pruneNested([...sel.keys()]).reduce((sum, p) => sum + (sel.get(p) ?? 0), 0);
}

/** FILETIME (100 ns since 1601) to a date. */
export const fileDate = (ft: number) => new Date(ft / 10_000 - 11_644_473_600_000);

export function age(ft: number, now = Date.now()): string {
  if (!ft) return "";
  const days = Math.floor((now - fileDate(ft).getTime()) / 86_400_000);
  if (days < 1) return "today";
  if (days < 60) return `${days} ${days === 1 ? "day" : "days"} ago`;
  const months = Math.round(days / 30);
  return months < 24 ? `${months} months ago` : `${Math.round(days / 365)} years ago`;
}

/** Inspection-sheet rows for each drive's free space. */
export function spaceChecks(drives: Drive[]): Check[] {
  return drives.map((d) => {
    const share = d.total ? d.free / d.total : 1;
    const mark = share < 0.05 ? "fix" : share < 0.1 ? "look" : "ok";
    return {
      system: `${letter(d.root)}: space`,
      reading: `${bytes(d.free)} free of ${bytes(d.total)}`,
      limit: "keep 10% free",
      mark,
      note:
        mark === "ok"
          ? "Plenty of room."
          : mark === "look"
            ? "Getting full. An SSD writes more slowly when it is nearly full."
            : "Almost full. Windows updates and game installs may fail.",
    };
  });
}

function shortModel(d: Disk): string {
  const name = d.model.replace(/^NVMe\s+/i, "").replace(/_/g, " ").replace(/\s+/g, " ").trim();
  return `${name.split(" ").slice(0, 2).join(" ")} ${d.media === "Unknown" ? "drive" : d.media}`;
}

/** Inspection-sheet rows for drive health; a row that says why when the service cannot be asked. */
export function healthChecks(health: Health | null, error: string | null): Check[] {
  if (!health) {
    return [
      {
        system: "Drive health",
        reading: error ? "Not available" : "Checking…",
        limit: "",
        mark: "none",
        note: error ? "Reading wear and temperature needs the Verity service." : "",
      },
    ];
  }
  return health.disks.map((d) => {
    const parts: string[] = [d.health];
    if (d.wear !== null) parts.push(`${d.wear}% worn`);
    if (d.temperature !== null) parts.push(`${d.temperature} °C`);
    const errors = (d.read_errors ?? 0) + (d.write_errors ?? 0);
    const hot = d.temperature !== null && d.temperature_max !== null && d.temperature >= d.temperature_max - 5;
    const mark =
      d.health === "Unhealthy" || errors > 0
        ? "fix"
        : d.health === "Warning" || (d.wear ?? 0) >= 80 || hot
          ? "look"
          : d.health === "Healthy"
            ? "ok"
            : "none";
    const note =
      mark === "fix"
        ? errors > 0
          ? `${errors.toLocaleString()} read or write errors. Back up what matters on this drive.`
          : "Windows reports this drive as failing. Back up what matters now."
        : mark === "look"
          ? hot
            ? "Running close to its temperature limit."
            : "Wearing out or warned by Windows. Keep backups current."
          : d.wear === null
            ? "This drive does not report wear or temperature."
            : d.wear === 0
              ? "Wear reads 0%; some drives report that even after years."
              : "No problems reported.";
    return {
      system: shortModel(d),
      reading: parts.join(" · "),
      limit: d.temperature_max !== null ? `limit ${d.temperature_max} °C` : "",
      mark,
      note,
    };
  });
}
