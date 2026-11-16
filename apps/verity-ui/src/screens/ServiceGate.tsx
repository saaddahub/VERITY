import { useState } from "react";
import { api } from "../api.ts";
import type { Data } from "../App.tsx";

/** Shown instead of a screen's content while the service can't answer. */
export function ServiceGate({ data }: { data: Data }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const s = data.service;
  if (!s) return <p className="hint">Checking the Verity service…</p>;

  const run = async (install: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await api.serviceSetup(install);
      data.refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = {
    missing: {
      title: "Verity's background service isn't installed yet",
      body: "It does the measuring for you. Installing it takes one Windows admin approval and changes nothing else on this PC.",
      action: "Install service",
      install: true,
    },
    stopped: {
      title: "Verity's background service is stopped",
      body: "Start it to see your checkup and record apps. Windows will ask for admin approval.",
      action: "Start service",
      install: false,
    },
    error: {
      title: "Verity can't reach its background service",
      body: s.state === "error" ? s.message : "",
      action: "Try again",
      install: false,
    },
    running: null,
  }[s.state];
  if (!copy) return null;

  return (
    <div className="banner" role="alert">
      <div>
        <h2>{copy.title}</h2>
        <p>{copy.body}</p>
        {error && <p className="error-text">{error}</p>}
      </div>
      <button
        className="btn primary"
        disabled={busy}
        onClick={() => (s.state === "error" ? data.refresh() : run(copy.install))}
      >
        {busy ? "Waiting for approval…" : copy.action}
      </button>
    </div>
  );
}
