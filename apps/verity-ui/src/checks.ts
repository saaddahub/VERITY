// Plain-language verdict and quick checks, from data Verity already measures.
// shortcut: these run in the UI from the last session and system info; when
// Doctor lands (M2) they become service rules (D001, D003, D010) and this file
// only renders their findings.

import type { Mark, SessionRow, SystemInfo } from "./api.ts";

export interface Verdict {
  mark: Mark;
  headline: string;
  detail: string;
}

export interface Check {
  system: string;
  reading: string;
  limit: string;
  mark: Mark;
  note: string;
}

const fps = (v: number) => v.toFixed(1);
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
export const appName = (exe: string) => exe.replace(/\.exe$/i, "");

export function verdict(s: SessionRow): Verdict {
  const { stats, exe, capture_s, warmup_excluded_s } = s.summary;
  const name = appName(exe);
  if (stats.frame_count === 0) {
    return {
      mark: "none",
      headline: `Too short to judge ${name}.`,
      detail: `Verity skips the first ${warmup_excluded_s.toFixed(0)} seconds while things settle. Record for longer.`,
    };
  }
  const groups = s.hitches.count;
  if (groups === 0) {
    return {
      mark: "ok",
      headline: `${name} ran smoothly at ${fps(stats.avg_fps)} FPS.`,
      detail: `No stutters in ${capture_s.toFixed(0)} seconds. Even the slowest 1% of frames held ${fps(stats.low_1pct_fps)} FPS.`,
    };
  }
  const worst = s.hitches.worst_ms;
  return {
    mark: stats.hitch_rate_per_min <= 3 ? "look" : "fix",
    headline: `${name} ran at ${fps(stats.avg_fps)} FPS but stuttered ${plural(groups, "time", "times")}.`,
    detail: `The worst frame took ${worst.toFixed(0)} ms, about ${stats.hitch_rate_per_min.toFixed(0)} stutters a minute.`,
  };
}

/** A GPU with its own memory, as opposed to graphics built into the CPU. */
export const isDiscrete = (g: { vendor: string; dedicated_vram_bytes: number }) =>
  (g.vendor === "NVIDIA" || g.vendor === "AMD") && g.dedicated_vram_bytes >= 2 ** 30;

const shortGpu = (name: string) =>
  name.replace(/\(R\)|\(TM\)/g, "").replace(/ Laptop GPU$/, "").replace(/\s+/g, " ").trim();

export function checks(s: SessionRow | null, info: SystemInfo | null): Check[] {
  const out: Check[] = [];
  if (info && s) {
    const discrete = info.gpus.find(isDiscrete);
    const used = s.summary.gpu_adapter;
    const usedGpu = info.gpus.find((g) => g.name === used);
    if (!used) {
      out.push({ system: "Graphics", reading: "Not seen", limit: "", mark: "none", note: "No GPU work was seen during the last recording." });
    } else if (discrete && usedGpu && !isDiscrete(usedGpu)) {
      out.push({ system: "Graphics", reading: shortGpu(used), limit: `${shortGpu(discrete.name)} available`, mark: "look", note: `${appName(s.summary.exe)} drew on the slower built-in GPU.` });
    } else {
      out.push({ system: "Graphics", reading: shortGpu(used), limit: "fastest in this PC", mark: "ok", note: "Ran on the best GPU this PC has." });
    }
  }
  if (info) {
    const d = info.displays[0];
    if (d) {
      const low = d.current_hz < d.max_hz;
      out.push({
        system: "Display",
        reading: `${d.current_hz} Hz`,
        limit: `max ${d.max_hz} Hz`,
        mark: low ? "look" : "ok",
        note: low ? `Can run at ${d.max_hz} Hz but is set lower.` : "Running at its highest refresh rate.",
      });
    }
    const p = info.power;
    const battery = p.battery_present && p.battery_percent !== null ? ` · ${p.battery_percent}%` : "";
    const plan = info.power_scheme.name ? `${info.power_scheme.name} plan` : "";
    if (p.ac_online === false) {
      out.push({ system: "Power", reading: `On battery${battery}`, limit: plan, mark: "fix", note: "Windows slows the PC down on battery. Plug in to play." });
    } else if (p.ac_online === true) {
      out.push({ system: "Power", reading: `Plugged in${battery}`, limit: plan, mark: "ok", note: "On mains power." });
    }
  }
  if (s) {
    const { overhead_cpu_pct: o, overhead_budget_pct: b } = s.summary;
    out.push({
      system: "Verity itself",
      reading: `${o.toFixed(2)}% CPU`,
      limit: `budget ${b.toFixed(2)}%`,
      mark: o <= b ? "ok" : "look",
      note: o <= b ? "Measuring cost almost nothing." : "Measuring cost more than planned on this run.",
    });
  }
  return out;
}

export const markLabel: Record<Mark, string> = { ok: "OK", look: "Worth a look", fix: "Fix it", none: "Not checked" };
