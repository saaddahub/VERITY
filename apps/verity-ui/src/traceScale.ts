// The frame-time chart's y axis, kept apart from the canvas code so it can be tested.

/** Axis values worth a label: the 60/30/20/15/10 FPS lines, then round numbers. */
const TICKS = [16.7, 33.3, 50, 66.7, 100, 150, 200, 250, 400, 600];
/** Closest two axis labels may sit, in pixels, before the second is skipped. */
const TICK_GAP = 16;

/** The y-axis top. One huge frame (a 17 s load) must not flatten the rest, so
 *  the scale follows the slowest 0.1% of frames, as the shared report does,
 *  and taller spikes run off the top. */
export function yScale(ms: (number | null)[]): { top: number; worst: number } {
  const sorted = ms.filter((m): m is number => m !== null).sort((a, b) => a - b);
  if (sorted.length === 0) return { top: 40, worst: 0 };
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * q))];
  // A game that is slow throughout keeps its whole line on the chart.
  const cap = Math.max(250, at(0.5) * 2);
  return { top: Math.min(cap, Math.max(40, at(0.999) * 1.15)), worst: sorted[sorted.length - 1] };
}

/** The ticks to draw, skipping any that would crowd the one below. */
export function ticksFor(top: number, plotH: number): number[] {
  const out: number[] = [];
  for (const ms of TICKS) {
    if (ms >= top) break;
    const prev = out[out.length - 1];
    if (prev === undefined || ((ms - prev) / top) * plotH >= TICK_GAP) out.push(ms);
  }
  return out;
}

/** 17797.8 → "17.8 s", 212.4 → "212 ms". */
export const duration = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms)} ms`);
