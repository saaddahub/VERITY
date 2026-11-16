import { useLayoutEffect, useRef } from "react";
import { animate, stagger } from "animejs";
import type { Mark } from "./api.ts";
import { markLabel, type Check } from "./checks.ts";

const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Icons are authored SVG in one 1.5px stroke. */
export function Icon({ name }: { name: "checkup" | "sessions" | "record" | "refresh" | "download" }) {
  const p = {
    checkup: "M4 5h12M4 10h12M4 15h7M14 14l2 2 3-4",
    sessions: "M4 5h13M4 10h13M4 15h13",
    record: "M10 4a6 6 0 1 0 0 12a6 6 0 1 0 0-12zM10 8a2 2 0 1 0 0 4a2 2 0 1 0 0-4z",
    refresh: "M15.5 7A6 6 0 1 0 16 12M16 4v3.5h-3.5",
    download: "M10 4v9M6 9.5l4 4 4-4M4.5 16h11",
  }[name];
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d={p} stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function Logo() {
  return (
    <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden="true">
      {[0, 1, 2].map((r) =>
        [0, 1, 2].map((c) => {
          const lit = (r === 0 && c === 0) || (r === 2 && c === 0) ? "#3fb27f" : r === 1 && c === 1 ? "#e8a33d" : "#262b33";
          return <rect key={`${r}${c}`} x={c * 7.5} y={r * 7.5} width="6" height="6" rx="1.5" fill={lit} />;
        }),
      )}
    </svg>
  );
}

/** The three-position inspection mark. Its final state is plain CSS, so the
 *  sheet reads correctly even if the animation never runs. */
export function MarkCells({ mark }: { mark: Mark }) {
  const cells: Mark[] = ["ok", "look", "fix"];
  return (
    <div className={`mark${mark === "none" ? " unchecked" : ""}`} role="img" aria-label={markLabel[mark]}>
      {cells.map((c) => (
        <span key={c} className={c === mark ? `on ${c}` : undefined}>
          <i data-fill={c === mark ? "" : undefined} />
          <em>{c === "ok" ? "OK" : c === "look" ? "Look" : "Fix"}</em>
        </span>
      ))}
    </div>
  );
}

/** The signature: the inspection sheet, whose marks tick into place row by row. */
export function Sheet({ rows, foot }: { rows: Check[]; foot: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const key = rows.map((r) => r.system + r.mark).join("|");
  useLayoutEffect(() => {
    if (!ref.current || reduced()) return;
    const fills = ref.current.querySelectorAll("[data-fill]");
    const anim = animate(fills, {
      opacity: { from: 0 },
      scaleX: { from: 0.2 },
      duration: 520,
      delay: stagger(90, { start: 140 }),
      ease: "out(4)",
    });
    return () => {
      anim.revert();
    };
  }, [key]);
  return (
    <div className="sheet" ref={ref}>
      <div className="sheet-row head colhead" aria-hidden="true">
        <span>System</span>
        <span>Reading</span>
        <span>Limit</span>
        <span>Check</span>
        <span>Note</span>
      </div>
      {rows.map((r) => (
        <div className="sheet-row" key={r.system}>
          <span className="sys">{r.system}</span>
          <span className="rd">{r.reading}</span>
          <span className="lim">{r.limit}</span>
          <MarkCells mark={r.mark} />
          <span className="note">{r.note}</span>
        </div>
      ))}
      <div className="sheet-foot">
        <span>{foot}</span>
        <span className="count">
          {rows.length} checks · {rows.filter((r) => r.mark === "look" || r.mark === "fix").length} need attention
        </span>
      </div>
    </div>
  );
}

export function Readings({ items }: { items: { label: string; value: string; unit?: string; word?: boolean }[] }) {
  return (
    <dl className="readings" style={{ margin: 0 }}>
      {items.map((i) => (
        <div key={i.label}>
          <dt className="label">{i.label}</dt>
          <dd style={{ margin: 0 }}>
            {i.word ? (
              <span className="word">{i.value}</span>
            ) : (
              <span className="v reading">
                {i.value}
                {i.unit && <span className="u">{i.unit}</span>}
              </span>
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export const when = (utc: string) =>
  new Date(utc).toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
