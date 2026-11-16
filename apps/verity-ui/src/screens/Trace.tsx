import { useEffect, useRef } from "react";
import { animate } from "animejs";
import type { HitchGroup, Trace as TraceData } from "../api.ts";

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Frame time over the recording: the line is min/max per pixel column so a
 *  3-hour session draws as fast as a 30-second one; stutters are amber ticks
 *  on a rail beneath. */
export function Trace({ data, hitches }: { data: TraceData; hitches: HitchGroup[] }) {
  const canvas = useRef<HTMLCanvasElement>(null);

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
      const sorted = data.ms.filter((m): m is number => m !== null).sort((a, b) => a - b);
      const worst = sorted[sorted.length - 1] ?? 1;
      const top = Math.max(40, worst * 1.12);
      const left = 44;
      const rail = 22;
      const plotH = h - rail - 18;
      const plotW = w - left - 8;
      const y = (ms: number) => 8 + plotH - (ms / top) * plotH;
      const x = (ms: number) => left + (ms / total) * plotW;

      // grid at the 60/30/20 FPS lines
      g.font = `500 11px ${css("--read")}`;
      g.textBaseline = "middle";
      for (const ms of [16.7, 33.3, 50, 66.7, 100].filter((m) => m < top)) {
        g.strokeStyle = css("--rule-soft");
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(left, Math.round(y(ms)) + 0.5);
        g.lineTo(w - 8, Math.round(y(ms)) + 0.5);
        g.stroke();
        g.fillStyle = css("--ink-3");
        g.fillText(`${ms.toFixed(0)}`, 6, y(ms));
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
      const railY = h - rail + 4;
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
      g.fillStyle = css("--look");
      for (const hg of hitches) {
        if (hg.start_qpc < qpc0) continue;
        const px = x(t[findIndex(hg.start_qpc)]);
        if (px > left + cols) continue;
        g.fillRect(Math.round(px) - 1, railY, 3, 13);
      }
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
  }, [data, hitches]);

  return <canvas ref={canvas} className="trace-canvas" role="img" aria-label="Frame time over the recording, with stutters marked beneath" />;
}
