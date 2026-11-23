import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Notice, type ServiceState, type Session, type SessionStatus, type SystemInfo } from "./api.ts";
import { Icon, Logo, type IconName } from "./ui.tsx";
import { Checkup } from "./screens/Checkup.tsx";
import { Sessions } from "./screens/Sessions.tsx";
import { Record } from "./screens/Record.tsx";
import { Games } from "./screens/Games.tsx";

export type Page = { name: "checkup" } | { name: "sessions"; id?: number } | { name: "record" } | { name: "games" };

export interface Data {
  service: ServiceState | null;
  info: SystemInfo | null;
  sessions: Session[] | null;
  error: string | null;
  refresh: () => void;
  session: SessionStatus | null;
  refreshSession: () => void;
  /** Unanswered "Is X a game?" questions. */
  asks: Notice[];
  answerAsk: (n: Notice, answer: "game" | "not_game" | "never") => void;
}

/** How often the app asks the service what Session Mode is doing. */
const POLL_MS = 2000;

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
      else if (name === "games") setPageState({ name });
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

  // Session Mode: poll status and collect notices newer than the last one seen.
  const [session, setSession] = useState<SessionStatus | null>(null);
  const [asks, setAsks] = useState<Notice[]>([]);
  const [toasts, setToasts] = useState<Notice[]>([]);
  const lastNotice = useRef<number | null>(null);
  const pollSession = useCallback(async () => {
    try {
      if (lastNotice.current === null) {
        // First poll: older notices are history, except unanswered questions.
        const all = await api.sessionStatus(0);
        setSession(all);
        lastNotice.current = all.notices.reduce((m, n) => Math.max(m, n.id), 0);
        setAsks(all.notices.filter((n) => n.kind === "ask_game"));
        return;
      }
      const st = await api.sessionStatus(lastNotice.current);
      setSession(st);
      if (st.notices.length === 0) return;
      lastNotice.current = st.notices.reduce((m, n) => Math.max(m, n.id), lastNotice.current);
      setAsks((a) => [...a, ...st.notices.filter((n) => n.kind === "ask_game")]);
      setToasts((t) => [...t, ...st.notices].slice(-4));
      if (st.notices.some((n) => n.kind === "session_end")) void refresh();
    } catch {
      setSession(null);
    }
  }, [refresh]);
  useEffect(() => {
    if (service?.state !== "running") return;
    void pollSession();
    const timer = setInterval(() => void pollSession(), POLL_MS);
    return () => clearInterval(timer);
  }, [service?.state, pollSession]);
  const answerAsk = (n: Notice, answer: "game" | "not_game" | "never") => {
    setAsks((a) => a.filter((x) => x.id !== n.id));
    setToasts((t) => t.filter((x) => x.id !== n.id));
    if (n.exe) void api.gamesAnswer(n.exe, answer);
  };
  const dismissToast = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);

  const data: Data = {
    service, info, sessions, error, refresh,
    session, refreshSession: () => void pollSession(), asks, answerAsk,
  };
  const nav: { page: Page["name"]; label: string; icon: IconName }[] = [
    { page: "checkup", label: "Checkup", icon: "checkup" },
    { page: "games", label: "Games", icon: "games" },
    { page: "sessions", label: "Sessions", icon: "sessions" },
    { page: "record", label: "Record", icon: "record" },
  ];
  const live = session?.status && session.status.state !== "idle" ? session.status : null;
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
          <span className="meta-line">
            {live
              ? `${live.state === "active" ? "Recording" : "Starting"} ${live.game.name}`
              : session
                ? `Session Mode ${session.status.enabled ? "on" : "off"}`
                : api.isPreview
                  ? "Design preview data"
                  : "v0.1.0"}
          </span>
        </div>
      </nav>
      <main className="work">
        <div className="work-inner">
          {page.name === "checkup" && <Checkup data={data} go={setPage} />}
          {page.name === "sessions" && <Sessions data={data} selected={page.id} go={setPage} />}
          {page.name === "record" && <Record data={data} go={setPage} />}
          {page.name === "games" && <Games data={data} go={setPage} />}
        </div>
      </main>
      <Toasts
        items={toasts}
        dismiss={dismissToast}
        open={(n) => {
          dismissToast(n.id);
          setPage(n.kind === "ask_game" ? { name: "games" } : { name: "sessions", id: n.session_id });
        }}
      />
    </div>
  );
}

/** Session start and end notices, bottom right. They leave by themselves after a while. */
function Toasts({ items, dismiss, open }: { items: Notice[]; dismiss: (id: number) => void; open: (n: Notice) => void }) {
  useEffect(() => {
    if (items.length === 0) return;
    const oldest = items[0];
    const timer = setTimeout(() => dismiss(oldest.id), oldest.kind === "info" ? 6000 : 12000);
    return () => clearTimeout(timer);
  }, [items, dismiss]);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {items.map((n) => (
        <div key={n.id} className={`toast ${n.kind}`}>
          <p>{n.kind === "ask_game" ? `Is ${n.exe} a game? Answer on the Games page.` : n.text}</p>
          {(n.kind === "session_end" && n.session_id !== undefined) || n.kind === "ask_game" ? (
            <button className="link" onClick={() => open(n)}>
              {n.kind === "ask_game" ? "Answer" : "Open session"}
            </button>
          ) : null}
          <button className="toast-x" onClick={() => dismiss(n.id)} aria-label="Dismiss">
            <Icon name="close" />
          </button>
        </div>
      ))}
    </div>
  );
}
