import { useCallback, useEffect, useState } from "react";
import { api, type GpuApp } from "../api.ts";
import type { Data, Page } from "../App.tsx";
import { appName } from "../checks.ts";
import { Icon } from "../ui.tsx";
import { ServiceGate } from "./ServiceGate.tsx";

const durations: { label: string; s: number | null }[] = [
  { label: "Until it closes", s: null },
  { label: "30 seconds", s: 30 },
  { label: "1 minute", s: 60 },
  { label: "2 minutes", s: 120 },
];

const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

export function Record({ data, go }: { data: Data; go: (p: Page) => void }) {
  const [apps, setApps] = useState<GpuApp[] | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [pid, setPid] = useState<number | null>(null);
  const [duration, setDuration] = useState<number | null>(null);
  const [recording, setRecording] = useState<{ name: string; started: number } | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const running = data.service?.state === "running";

  const scan = useCallback(async () => {
    setApps(null);
    setScanError(null);
    try {
      const list = await api.gpuApps();
      setApps(list);
      setPid((p) => (list.some((a) => a.pid === p) ? p : list[0]?.pid ?? null));
    } catch (e) {
      setScanError(String(e));
    }
  }, []);

  useEffect(() => {
    if (running) void scan();
  }, [running, scan]);

  useEffect(() => {
    if (!recording) return;
    const t = setInterval(() => setElapsed((Date.now() - recording.started) / 1000), 250);
    return () => clearInterval(t);
  }, [recording]);

  if (!running) return <ServiceGate data={data} />;

  const start = async () => {
    const app = apps?.find((a) => a.pid === pid);
    if (!app) return;
    setError(null);
    setElapsed(0);
    setRecording({ name: appName(app.name), started: Date.now() });
    try {
      const session = await api.record(app.pid, duration);
      data.refresh();
      go({ name: "sessions", id: session.id });
    } catch (e) {
      setError(String(e));
    } finally {
      setRecording(null);
    }
  };

  if (recording) {
    return (
      <div className="panel recording" role="status" aria-live="polite">
        <span className="hint">
          <span className="pulse" aria-hidden="true" />
          Recording {recording.name}
        </span>
        <span className="clock reading">
          {clock(elapsed)}
          {duration !== null && <span className="hint"> of {clock(duration)}</span>}
        </span>
        <p style={{ margin: 0, color: "var(--ink-2)" }}>
          {duration === null
            ? `Keep playing. The recording ends when you close ${recording.name}.`
            : "Keep playing. The results open here when the time is up."}
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="page-head">
        <h1>Record an app</h1>
        <p>Pick a game or app that's running now. Verity only watches its frames; it never opens or changes the app.</p>
      </div>
      <div className="record">
        <section className="panel" aria-labelledby="apps-h">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "14px 16px 6px" }}>
            <h2 id="apps-h" className="section-title">
              Apps using the GPU now
            </h2>
            <button className="btn" onClick={scan} disabled={apps === null && !scanError}>
              <Icon name="refresh" />
              Refresh
            </button>
          </div>
          {scanError ? (
            <p className="error-text" style={{ padding: "0 16px 16px" }}>
              Couldn't list apps: {scanError}
            </p>
          ) : apps === null ? (
            <p className="hint" style={{ padding: "8px 16px 18px" }}>
              Looking for apps that are drawing frames…
            </p>
          ) : apps.length === 0 ? (
            <div className="empty">
              <h2>Nothing is drawing frames</h2>
              <p>Start your game or app, give it a few seconds, then refresh.</p>
            </div>
          ) : (
            <div className="apps" role="radiogroup" aria-label="App to record">
              {apps.map((a) => (
                <label className="app" key={a.pid}>
                  <input type="radio" name="app" checked={pid === a.pid} onChange={() => setPid(a.pid)} />
                  <span>
                    <span className="name">{appName(a.name)}</span>
                    <span className="sub reading" style={{ display: "block" }}>
                      {a.name} · pid {a.pid}
                    </span>
                  </span>
                  <span className="gpu reading">{a.gpu_pct.toFixed(0)}% GPU</span>
                </label>
              ))}
            </div>
          )}
        </section>
        <section className="panel choose" aria-labelledby="dur-h">
          <h2 id="dur-h" className="section-title">How long</h2>
          <div className="seg" role="radiogroup" aria-labelledby="dur-h">
            {durations.map((d) => (
              <label key={d.label}>
                <input type="radio" name="dur" checked={duration === d.s} onChange={() => setDuration(d.s)} />
                {d.label}
              </label>
            ))}
          </div>
          <button className="btn primary" style={{ justifyContent: "center" }} disabled={pid === null} onClick={start}>
            Start recording
          </button>
          {error && <p className="error-text">{error}</p>}
          <p className="hint" style={{ margin: 0 }}>
            The first 5 seconds are left out of the results while things settle.
          </p>
        </section>
      </div>
    </>
  );
}
