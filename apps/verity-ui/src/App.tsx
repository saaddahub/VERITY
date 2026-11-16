import { useCallback, useEffect, useState } from "react";
import { api, type ServiceState, type Session, type SystemInfo } from "./api.ts";
import { Icon, Logo } from "./ui.tsx";
import { Checkup } from "./screens/Checkup.tsx";
import { Sessions } from "./screens/Sessions.tsx";
import { Record } from "./screens/Record.tsx";

export type Page = { name: "checkup" } | { name: "sessions"; id?: number } | { name: "record" };

export interface Data {
  service: ServiceState | null;
  info: SystemInfo | null;
  sessions: Session[] | null;
  error: string | null;
  refresh: () => void;
}

const serviceLine: Record<ServiceState["state"], { dot: string; label: string }> = {
  running: { dot: "ok", label: "Service running" },
  stopped: { dot: "look", label: "Service stopped" },
  missing: { dot: "fix", label: "Service not installed" },
  error: { dot: "fix", label: "Service unreachable" },
};

export function App() {
  const [page, setPageState] = useState<Page>({ name: "checkup" });
  // The page lives in the URL hash (#sessions/4, #record) so screens can be linked.
  useEffect(() => {
    const read = () => {
      const [name, id] = location.hash.slice(1).split("/");
      if (name === "sessions") setPageState({ name, id: id ? Number(id) : undefined });
      else if (name === "record") setPageState({ name });
      else setPageState({ name: "checkup" });
    };
    read();
    addEventListener("hashchange", read);
    return () => removeEventListener("hashchange", read);
  }, []);
  const setPage = (p: Page) => {
    location.hash = p.name === "sessions" && p.id !== undefined ? `sessions/${p.id}` : p.name;
  };
  const [service, setService] = useState<ServiceState | null>(null);
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const status = await api.serviceStatus();
    setService(status);
    if (status.state !== "running") return;
    try {
      const [i, s] = await Promise.all([api.systemInfo(), api.sessions()]);
      setInfo(i);
      setSessions(s);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const data: Data = { service, info, sessions, error, refresh };
  const nav: { page: Page["name"]; label: string; icon: "checkup" | "sessions" | "record" }[] = [
    { page: "checkup", label: "Checkup", icon: "checkup" },
    { page: "sessions", label: "Sessions", icon: "sessions" },
    { page: "record", label: "Record", icon: "record" },
  ];
  const line = service ? serviceLine[service.state] : null;

  return (
    <div className="shell">
      <nav className="rail" aria-label="Main">
        <div className="brand">
          <Logo />
          <span>Verity</span>
        </div>
        {nav.map((n) => (
          <button
            key={n.page}
            className="nav"
            aria-current={page.name === n.page ? "page" : undefined}
            aria-label={n.label}
            onClick={() => setPage({ name: n.page } as Page)}
          >
            <Icon name={n.icon} />
            <span>{n.label}</span>
          </button>
        ))}
        <div className="svc" role="status" aria-live="polite">
          <b>
            <span className={`dot ${line?.dot ?? ""}`} />
            <span className="svc-label">{line?.label ?? "Checking service…"}</span>
          </b>
          <span className="meta-line">{api.isPreview ? "Design preview data" : "Offline · v0.1.0"}</span>
        </div>
      </nav>
      <main className="work">
        <div className="work-inner">
          {page.name === "checkup" && <Checkup data={data} go={setPage} />}
          {page.name === "sessions" && <Sessions data={data} selected={page.id} go={setPage} />}
          {page.name === "record" && <Record data={data} go={setPage} />}
        </div>
      </main>
    </div>
  );
}
