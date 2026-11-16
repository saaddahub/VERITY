import type { Data, Page } from "../App.tsx";
import { appName, checks, verdict } from "../checks.ts";
import { Readings, Sheet, when } from "../ui.tsx";
import { ServiceGate } from "./ServiceGate.tsx";
import { Headline } from "./Headline.tsx";
import type { SystemInfo } from "../api.ts";

const tidy = (s: string) => s.replace(/\(R\)|\(TM\)/g, "").replace(/\s+/g, " ").trim();

function ThisPc({ info }: { info: SystemInfo }) {
  const w = info.windows;
  const d = info.displays[0];
  const kinds = info.cpu.logical.map((l) => l.kind);
  const perf = kinds.filter((k) => k === "performance").length;
  const cores = perf > 0 ? `${perf} performance + ${kinds.length - perf} efficiency threads` : `${kinds.length} threads`;
  const items = [
    ["Windows", `${w.build >= 22000 ? "Windows 11" : "Windows 10"}${w.display_version ? ` ${w.display_version}` : ""}`, `build ${w.build}.${w.ubr}`],
    ["Processor", tidy(info.cpu.model).replace(/ @ .*$/, ""), cores],
    ...info.gpus.map((g) => ["Graphics", tidy(g.name), `${(g.dedicated_vram_bytes / 2 ** 30).toFixed(1)} GB own memory`]),
    ...(d ? [["Display", `${d.width} × ${d.height}`, `${d.current_hz} Hz of ${d.max_hz} Hz`]] : []),
  ];
  return (
    <section className="panel" aria-labelledby="pc-h">
      <h2 id="pc-h" className="section-title" style={{ padding: "16px 20px 2px" }}>
        This PC
      </h2>
      <dl className="facts" style={{ margin: 0 }}>
        {items.map(([k, v, sub]) => (
          <div key={k + v}>
            <dt>{k}</dt>
            <dd>
              {v}
              <span className="hint" style={{ display: "block" }}>
                {sub}
              </span>
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

export function Checkup({ data, go }: { data: Data; go: (p: Page) => void }) {
  if (data.service?.state !== "running") return <ServiceGate data={data} />;
  if (data.error) return <p className="error-text">{data.error}</p>;
  if (!data.sessions || !data.info) return <p className="hint">Reading this PC…</p>;

  const last = data.sessions[0] ?? null;
  const rows = checks(last, data.info);

  return (
    <>
      {last ? (
        <section className="verdict" aria-labelledby="verdict">
          <div>
            <Headline id="verdict" v={verdict(last)} />
            <div className="meta">
              Last recording · {appName(last.summary.exe)} · {when(last.started_utc)} · {last.summary.capture_s.toFixed(1)} s
            </div>
          </div>
          <div className="actions">
            <button className="btn" onClick={() => go({ name: "sessions", id: last.id })}>
              Open session
            </button>
            <button className="btn primary" onClick={() => go({ name: "record" })}>
              Record again
            </button>
          </div>
        </section>
      ) : (
        <section className="verdict">
          <div>
            <h1>Record something to get your first verdict.</h1>
            <p>Start a game or app, then let Verity watch it. Recording never changes anything on this PC.</p>
          </div>
          <div className="actions">
            <button className="btn primary" onClick={() => go({ name: "record" })}>
              Record an app
            </button>
          </div>
        </section>
      )}

      {last && last.summary.stats.frame_count > 0 && (
        <Readings
          items={[
            { label: "Average", value: last.summary.stats.avg_fps.toFixed(1), unit: "FPS" },
            { label: "Slowest 1%", value: last.summary.stats.low_1pct_fps.toFixed(1), unit: "FPS" },
            { label: "Slowest 0.1%", value: last.summary.stats.low_01pct_fps.toFixed(1), unit: "FPS" },
            { label: "Stutters", value: String(last.summary.stats.hitch_groups.length) },
          ]}
        />
      )}

      <Sheet
        rows={rows}
        foot={last ? "From the last recording and this PC as it is right now" : "From this PC as it is right now"}
      />

      <ThisPc info={data.info} />
    </>
  );
}
