import { useEffect, useState } from "react";
import { api, type Game, type Notice } from "../api.ts";
import type { Data, Page } from "../App.tsx";
import { Icon } from "../ui.tsx";
import { ServiceGate } from "./ServiceGate.tsx";

const sourceName: Record<Game["source"], string> = {
  steam: "Steam",
  epic: "Epic Games",
  gog: "GOG",
  xbox: "Xbox app",
  manual: "Added by you",
};

const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

export function Games({ data, go }: { data: Data; go: (p: Page) => void }) {
  const [games, setGames] = useState<Game[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [path, setPath] = useState("");

  useEffect(() => {
    if (data.service?.state === "running") api.games().then(setGames, (e) => setMessage({ text: String(e), error: true }));
  }, [data.service?.state]);

  if (data.service?.state !== "running") return <ServiceGate data={data} />;

  const run = async (label: string, f: () => Promise<void>) => {
    setBusy(label);
    setMessage(null);
    try {
      await f();
    } catch (e) {
      setMessage({ text: String(e).replace(/^Error: /, ""), error: true });
    } finally {
      setBusy(null);
    }
  };

  const st = data.session?.status;
  const enabled = st?.enabled ?? false;
  const recording = st && st.state !== "idle" ? st : null;

  let heading: React.ReactNode = "Checking Session Mode…";
  let meta = "";
  if (st?.state === "idle") {
    heading = enabled ? (
      <>Session Mode is <span className="hl-ok">on</span> and waiting for a game.</>
    ) : (
      <>Session Mode is <span className="hl-look">off</span>.</>
    );
    meta = enabled ? "Start any game in the library below and Verity records it." : "Games you start are not recorded until you turn it back on.";
  } else if (st?.state === "arming") {
    heading = <>{st.game.name} just started.</>;
    meta = "Recording counts once it has run for 10 seconds and drawn its first frames, so launchers and crashes are skipped.";
  } else if (st?.state === "active") {
    heading = st.exited_at ? <>{st.game.name} closed.</> : <>Recording {st.game.name}.</>;
    meta = st.exited_at
      ? "Saving in a few seconds, unless the game starts again."
      : `${(st.frames ?? 0).toLocaleString()} frames so far · stops when the game closes`;
  }

  const toggle = () =>
    run("toggle", async () => {
      await api.sessionEnable(!enabled);
      data.refreshSession();
    });
  const stop = () =>
    run("stop", async () => {
      await api.sessionStop();
      data.refreshSession();
    });
  const panic = () =>
    run("panic", async () => {
      await api.sessionPanic();
      data.refreshSession();
      setMessage({ text: "Done. Any session has ended and Session Mode is off. Turn it back on when you are ready." });
    });
  const scan = () =>
    run("scan", async () => {
      const g = await api.gamesScan();
      setGames(g);
      setMessage({ text: `Found ${g.length} ${g.length === 1 ? "game" : "games"}.` });
    });
  const add = (e: React.FormEvent) => {
    e.preventDefault();
    if (!path.trim()) return;
    void run("add", async () => {
      await api.gamesAdd(path);
      setPath("");
      setGames(await api.games());
      setMessage({ text: "Added. Verity records it the next time it starts." });
    });
  };
  const remove = (g: Game) =>
    run(`remove-${g.id}`, async () => {
      await api.gamesRemove(g.id);
      setGames(await api.games());
    });

  return (
    <>
      <section className="verdict" aria-labelledby="gh">
        <div>
          <h1 id="gh" aria-live="polite">
            {heading}
          </h1>
          <p>
            Verity records every game in this list while you play, then shows how smoothly it ran and what each stutter
            coincides with. It does not change any Windows settings.
          </p>
          {meta && <div className="meta">{meta}</div>}
        </div>
        <div className="actions">
          <button
            className="switch"
            role="switch"
            aria-checked={enabled}
            onClick={toggle}
            disabled={busy !== null || !st}
          >
            <span className="track" aria-hidden="true">
              <span className="knob" />
            </span>
            Session Mode
          </button>
          {recording && (
            <button className="btn" onClick={stop} disabled={busy !== null}>
              <Icon name="stop" />
              {busy === "stop" ? "Stopping…" : "Stop and save"}
            </button>
          )}
          <button className="btn danger" onClick={panic} disabled={busy !== null}>
            <Icon name="undo" />
            {busy === "panic" ? "Undoing…" : "Undo now"}
            <kbd>Ctrl Alt Shift V</kbd>
          </button>
        </div>
      </section>

      {data.asks.map((n) => (
        <Ask key={n.id} notice={n} answer={data.answerAsk} />
      ))}
      {data.session?.capabilities.map((c) => (
        <p key={c} className="error-text">
          {c}
        </p>
      ))}
      {message && (
        <p className={message.error ? "error-text" : "hint"} role="status">
          {message.text}
        </p>
      )}

      <section className="glib" aria-labelledby="lib">
        <div className="glib-head">
          <h2 id="lib" className="section-title">
            Game library {games && <span className="count reading">{games.length}</span>}
          </h2>
          <button className="btn" onClick={scan} disabled={busy !== null}>
            <Icon name="refresh" />
            {busy === "scan" ? "Scanning…" : "Scan again"}
          </button>
        </div>
        <div className="sheet">
          <div className="grow head colhead" aria-hidden="true">
            <span>Game</span>
            <span>Found in</span>
            <span>Program</span>
            <span />
          </div>
          {games === null && <p className="hint grow-msg">Loading the library…</p>}
          {games?.length === 0 && (
            <p className="grow-msg">
              No games found yet. Verity looks in Steam, Epic Games, GOG and Xbox app folders. Add one by its program
              file below.
            </p>
          )}
          {games?.map((g) => (
            <div className="grow" key={g.id}>
              <span className="gname">{g.name}</span>
              <span className="gsrc">{sourceName[g.source]}</span>
              <span className="gexe reading" title={g.exe_path}>
                {fileName(g.exe_path)}
              </span>
              <button
                className="link"
                onClick={() => remove(g)}
                disabled={busy !== null}
                aria-label={`Remove ${g.name} from the library`}
              >
                Remove
              </button>
            </div>
          ))}
          <form className="gadd" onSubmit={add}>
            <label htmlFor="gpath" className="sr">
              Full path to a game's .exe file
            </label>
            <input
              id="gpath"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="Paste the full path to a game's .exe, like D:\Games\Game\game.exe"
              spellCheck={false}
              autoComplete="off"
            />
            <button className="btn" type="submit" disabled={busy !== null || !path.trim()}>
              {busy === "add" ? "Adding…" : "Add game"}
            </button>
          </form>
        </div>
        <p className="hint">
          Past recordings are on <button className="link" onClick={() => go({ name: "sessions" })}>Sessions</button>.
          Closing the window keeps Verity in the tray so <kbd>Ctrl Alt Shift V</kbd> still works.
        </p>
      </section>
    </>
  );
}

function Ask({ notice, answer }: { notice: Notice; answer: (n: Notice, a: "game" | "not_game" | "never") => void }) {
  return (
    <section className="banner" aria-labelledby={`ask-${notice.id}`}>
      <div>
        <h2 id={`ask-${notice.id}`}>Is {notice.exe} a game?</h2>
        <p>It has been in front, drawing steady 3D work, for 10 seconds. Answer once and Verity remembers.</p>
      </div>
      <div className="actions">
        <button className="btn primary" onClick={() => answer(notice, "game")}>
          Yes, record it
        </button>
        <button className="btn" onClick={() => answer(notice, "not_game")}>
          No
        </button>
        <button className="btn" onClick={() => answer(notice, "never")}>
          Never ask
        </button>
      </div>
    </section>
  );
}
