import { useEffect, useMemo, useState } from "react";
import { api, type Session, type StoryData, type SystemInfo, type Trace as TraceData } from "../api.ts";
import type { Data, Page } from "../App.tsx";
import { appName, checks, verdict } from "../checks.ts";
import { Icon, Readings, Sheet, when } from "../ui.tsx";
import { ServiceGate } from "./ServiceGate.tsx";
import { Headline } from "./Headline.tsx";
import { Trace, type Lane } from "./Trace.tsx";
import { StoryPanel, kindLabel } from "./Story.tsx";

/** Lanes for the trace: reported causes first, then the busiest other kinds. */
function lanesFrom(story: StoryData | null): Lane[] {
  if (!story) return [];
  const causeKinds = new Set(story.story.causes.map((c) => c.kind));
  const byKind = new Map<string, Lane["events"]>();
  for (const e of story.events) {
    if (!kindLabel[e.kind]) continue;
    const list = byKind.get(e.kind) ?? [];
    list.push({ qpc: e.qpc, ticks: e.duration_ticks });
    byKind.set(e.kind, list);
  }
  return [...byKind.entries()]
    .sort((a, b) => Number(causeKinds.has(b[0])) - Number(causeKinds.has(a[0])) || b[1].length - a[1].length)
    .slice(0, 6)
    .map(([kind, events]) => ({ label: kindLabel[kind], cause: causeKinds.has(kind), events }));
}

export function Sessions({ data, selected, go }: { data: Data; selected?: number; go: (p: Page) => void }) {
  if (data.service?.state !== "running") return <ServiceGate data={data} />;
  if (data.error) return <p className="error-text">{data.error}</p>;
  if (!data.sessions) return <p className="hint">Loading sessions…</p>;
  if (data.sessions.length === 0) {
    return (
      <div className="panel empty">
        <h2>No recordings yet</h2>
        <p>Record a game or app and its results land here, newest first.</p>
        <button className="btn primary" onClick={() => go({ name: "record" })}>
          Record an app
        </button>
      </div>
    );
  }
  const current = data.sessions.find((s) => s.id === selected) ?? data.sessions[0];

  return (
    <div className="sessions">
      <div className="panel slist" role="list" aria-label="Recordings">
        {data.sessions.map((s) => {
          const v = verdict(s);
          return (
            <button
              key={s.id}
              role="listitem"
              className="srow"
              aria-current={s.id === current.id ? "true" : undefined}
              onClick={() => go({ name: "sessions", id: s.id })}
            >
              <span className={`dot ${v.mark}`} aria-hidden="true" />
              <span>
                <span className="name">{appName(s.summary.exe)}</span>
                <span className="when" style={{ display: "block" }}>
                  #{s.id} · {when(s.started_utc)}
                </span>
              </span>
              <span className="fps reading">
                {s.summary.stats.avg_fps.toFixed(1)}
                <small>{s.summary.stats.hitch_groups.length} stutters</small>
              </span>
            </button>
          );
        })}
      </div>
      <Detail key={current.id} session={current} info={data.info} />
    </div>
  );
}

function Detail({ session, info }: { session: Session; info: SystemInfo | null }) {
  const [trace, setTrace] = useState<TraceData | null>(null);
  const [traceError, setTraceError] = useState<string | null>(null);
  const [exported, setExported] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [story, setStory] = useState<StoryData | null>(null);
  const [storyError, setStoryError] = useState<string | null>(null);
  const lanes = useMemo(() => lanesFrom(story), [story]);
  const s = session.summary;
  const st = s.stats;

  useEffect(() => {
    api.trace(session.id, session.frame_chunks).then(setTrace, (e) => setTraceError(String(e)));
    api.story(session.id).then(setStory, (e) => setStoryError(String(e)));
  }, [session.id, session.frame_chunks]);

  const exportCsv = async () => {
    setExporting(true);
    try {
      setExported(`Saved to ${await api.exportCsv(session.id, session.frame_chunks)}`);
    } catch (e) {
      setExported(`Couldn't save the file: ${String(e)}`);
    } finally {
      setExporting(false);
    }
  };

  return (
    <div style={{ display: "grid", gap: 24 }}>
      <section className="verdict" aria-labelledby="sv">
        <div>
          <Headline id="sv" v={verdict(session)} />
          <div className="meta">
            Recording #{session.id} · {when(session.started_utc)} · {s.capture_s.toFixed(1)} s · first {s.warmup_excluded_s.toFixed(0)} s left out
          </div>
        </div>
        <div className="actions">
          <button className="btn" onClick={exportCsv} disabled={exporting}>
            <Icon name="download" />
            {exporting ? "Saving…" : "Export CSV"}
          </button>
        </div>
      </section>
      {exported && (
        <p className="hint" role="status">
          {exported}
        </p>
      )}

      {st.frame_count > 0 && (
        <Readings
          items={[
            { label: "Average", value: st.avg_fps.toFixed(1), unit: "FPS" },
            { label: "Slowest 1%", value: st.low_1pct_fps.toFixed(1), unit: "FPS" },
            { label: "Slowest 0.1%", value: st.low_01pct_fps.toFixed(1), unit: "FPS" },
            { label: "Stutters", value: String(st.hitch_groups.length), unit: st.hitch_groups.length ? `${st.hitch_rate_per_min.toFixed(1)}/min` : undefined },
          ]}
        />
      )}

      <Sheet
        rows={checks(session, info).filter((c) => c.system === "Graphics" || c.system === "Verity itself")}
        foot="Checked against this recording"
      />

      {st.frame_count > 0 && <StoryPanel data={story} error={storyError} />}

      <section className="panel trace-panel" aria-labelledby="th">
        <div className="trace-head">
          <h2 id="th">Frame time</h2>
          <div className="legend">
            <span>
              <i /> milliseconds per frame
            </span>
            <span>
              <i className="h" /> stutter
            </span>
          </div>
        </div>
        {trace ? (
          <Trace data={trace} hitches={st.hitch_groups} lanes={lanes} freq={story?.qpc_freq} />
        ) : traceError ? (
          <p className="error-text">Couldn't load the frames: {traceError}</p>
        ) : (
          <div className="trace-canvas" aria-busy="true" />
        )}
      </section>

      <dl className="panel facts" style={{ margin: 0 }}>
        <div>
          <dt>Frames recorded</dt>
          <dd className="reading">{s.total_frames.toLocaleString()}</dd>
        </div>
        <div>
          <dt>Frame pacing spread</dt>
          <dd className="reading">{st.pacing_spread_ms.toFixed(2)} ms</dd>
        </div>
        <div>
          <dt>Length counted</dt>
          <dd className="reading">{st.duration_s.toFixed(1)} s</dd>
        </div>
        {s.capture_gaps.length > 0 && (
          <div>
            <dt>Gaps in the recording</dt>
            <dd>{s.capture_gaps.length} short gap, not counted as stutter</dd>
          </div>
        )}
      </dl>
    </div>
  );
}
