import { useEffect, useMemo, useRef } from "react";
import { animate } from "animejs";
import type { HitchGroup, Trace as TraceData } from "../api.ts";
import { duration, ticksFor, yScale } from "../traceScale.ts";

/** One row of system events under the trace, sharing its time axis. */
export interface Lane {
  label: string;
  /** Drawn in the attention colour: this kind is a reported cause. */
  cause: boolean;
  events: { qpc: number; ticks: number }[];
}

const LANE_H = 18;

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Frame time over the recording: the line is min/max per pixel column so a
 *  3-hour session draws as fast as a 30-second one; stutters are amber ticks
 *  on a rail beneath. */
export function Trace({ data, hitches, lanes = [], freq = 10_000_000 }: { data: TraceData; hitches: HitchGroup[]; lanes?: Lane[]; freq?: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { top, worst } = useMemo(() => yScale(data.ms), [data]);

  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    let reveal = 1;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = el.clientWidth;
      const h = el.clientHeight;
      el.width = Math.round(w * dpr);
      el.height = Math.round(h * dpr);
      const g = el.getContext("2d");
      if (!g) return;
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, w, h);

      const n = data.ms.length;
      if (n < 2) return;
      // x axis: elapsed time from summed frame times (consecutive frames).
      const t: number[] = new Array(n);
      let acc = 0;
      for (let i = 0; i < n; i++) {
        t[i] = acc;
        acc += data.ms[i] ?? 0;
      }
      const total = acc || 1;
      const left = lanes.length ? 168 : 44;
      const rail = 22;
      const lanesH = lanes.length ? lanes.length * LANE_H + 10 : 0;
      const plotH = h - rail - 18 - lanesH;
      const plotW = w - left - 8;
      // Frames slower than the top are drawn reaching the top edge.
      const y = (ms: number) => 8 + plotH - (Math.min(ms, top) / top) * plotH;
      const x = (ms: number) => left + (ms / total) * plotW;

      // grid at the 60/30/20 FPS lines and round numbers, never crowded
      g.font = `500 11px ${css("--read")}`;
      g.textBaseline = "middle";
      for (const ms of ticksFor(top, plotH)) {
        g.strokeStyle = css("--rule-soft");
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(left, Math.round(y(ms)) + 0.5);
        g.lineTo(w - 8, Math.round(y(ms)) + 0.5);
        g.stroke();
        g.fillStyle = css("--ink-3");
        g.fillText(`${ms.toFixed(0)}`, left - 38, y(ms));
      }

      // the line, min/max per pixel column, revealed left to right
      const cols = Math.max(1, Math.floor(plotW * reveal));
      const lo = new Float32Array(cols).fill(Infinity);
      const hi = new Float32Array(cols).fill(-Infinity);
      for (let i = 0; i < n; i++) {
        const m = data.ms[i];
        if (m === null) continue;
        const c = Math.floor(((t[i] / total) * plotW));
        if (c >= cols) break;
        if (m < lo[c]) lo[c] = m;
        if (m > hi[c]) hi[c] = m;
      }
      g.strokeStyle = css("--ink-2");
      g.lineWidth = 1.25;
      g.beginPath();
      let started = false;
      for (let c = 0; c < cols; c++) {
        if (hi[c] === -Infinity) continue;
        const px = left + c + 0.5;
        if (!started) {
          g.moveTo(px, y(lo[c]));
          started = true;
        } else {
          g.lineTo(px, y(lo[c]));
        }
        if (hi[c] !== lo[c]) g.lineTo(px, y(hi[c]));
      }
      g.stroke();

      // stutter rail
      const railY = h - rail + 4 - lanesH;
      g.fillStyle = css("--rule-soft");
      g.fillRect(left, railY + 6, plotW, 1);
      const qpc0 = data.qpc[0];
      const findIndex = (q: number) => {
        let a = 0;
        let b = n - 1;
        while (a < b) {
          const mid = (a + b) >> 1;
          if (data.qpc[mid] < q) a = mid + 1;
          else b = mid;
        }
        return a;
      };
      if (lanes.length) {
        g.font = `500 12px ${css("--text")}`;
        g.fillStyle = css("--ink-2");
        g.textBaseline = "middle";
        g.fillText("Stutters", 4, railY + 6);
      }
      g.fillStyle = css("--look");
      for (const hg of hitches) {
        if (hg.start_qpc < qpc0) continue;
        const px = x(t[findIndex(hg.start_qpc)]);
        if (px > left + cols) continue;
        g.fillRect(Math.round(px) - 1, railY, 3, 13);
      }

      // event lanes: same time axis, so an event sits under the frame it overlapped
      // Position of a QPC time: the next frame's place, stepped back by the gap to it.
      const qpcX = (q: number) => {
        const i = findIndex(q);
        return x(Math.max(0, t[i] - (Math.max(0, data.qpc[i] - q) / freq) * 1000));
      };
      lanes.forEach((lane, i) => {
        const ly = h - lanesH + 4 + i * LANE_H;
        g.fillStyle = css("--rule-soft");
        g.fillRect(left, ly + LANE_H / 2, plotW, 1);
        g.font = `500 12px ${css("--text")}`;
        g.fillStyle = lane.cause ? css("--ink") : css("--ink-3");
        g.textBaseline = "middle";
        let label = lane.label;
        while (g.measureText(label).width > left - 16 && label.length > 4) label = label.slice(0, -2) + "…";
        g.fillText(label, 4, ly + LANE_H / 2);
        g.fillStyle = lane.cause ? css("--fix") : css("--ink-3");
        for (const e of lane.events) {
          if (e.qpc < qpc0 || e.qpc > data.qpc[n - 1]) continue;
          const a = qpcX(e.qpc);
          if (a > left + cols) continue;
          const b = e.ticks > 0 ? qpcX(e.qpc + e.ticks) : a;
          g.fillRect(Math.round(a), ly + 4, Math.max(2, Math.round(b - a)), LANE_H - 8);
        }
      });
    };

    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(el);
    let anim: ReturnType<typeof animate> | null = null;
    if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      const state = { r: 0 };
      anim = animate(state, {
        r: 1,
        duration: 900,
        ease: "out(3)",
        onUpdate: () => {
          reveal = state.r;
          draw();
        },
      });
    }
    return () => {
      ro.disconnect();
      anim?.pause();
    };
  }, [data, hitches, lanes, freq, top]);

  const cut = worst > top;
  return (
    <>
      <canvas
        ref={canvas}
        className="trace-canvas"
        style={lanes.length ? { height: 260 + lanes.length * LANE_H + 10 } : undefined}
        role="img"
        aria-label={
          lanes.length
            ? `Frame time over the recording, stutters marked beneath, then ${lanes.length} rows of system events: ${lanes.map((l) => l.label).join(", ")}`
            : "Frame time over the recording, with stutters marked beneath"
        }
      />
      {cut && (
        <p className="hint">
          Frames slower than {Math.round(top)} ms run off the top of the chart. The slowest took {duration(worst)}.
        </p>
      )}
    </>
  );
}
