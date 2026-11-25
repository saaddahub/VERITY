import { useCallback, useEffect, useState } from "react";
import { api, changes, type ActionInfo, type Game, type JournalRow, type Plan, type StartupItem } from "../api.ts";
import type { Data } from "../App.tsx";
import { friendly, group, itemName, phaseLabel, when, type Group } from "../changes.ts";
import { Icon } from "../ui.tsx";
import { ServiceGate } from "./ServiceGate.tsx";

/** A change waiting for the user to accept going on without a restore point. */
interface Consent {
  reason: string;
  id: string;
  params: Record<string, unknown>;
  label: string;
}

const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

export function Changes({ data }: { data: Data }) {
  const [catalog, setCatalog] = useState<ActionInfo[]>([]);
  const [rows, setRows] = useState<JournalRow[] | null>(null);
  const [plans, setPlans] = useState<Record<string, Plan>>({});
  const [startup, setStartup] = useState<StartupItem[] | null>(null);
  const [games, setGames] = useState<Game[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [consent, setConsent] = useState<Consent | null>(null);
  const running = data.service?.state === "running";

  const load = useCallback(async () => {
    const [cat, list, items] = await Promise.all([changes.catalog(), changes.list(), changes.startupItems()]);
    setCatalog(cat);
    setRows(list);
    setStartup(items);
    const ids = ["gamemode.enable", "display.max_refresh"];
    const got = await Promise.all(ids.map((id) => changes.plan(id).catch((e) => ({ change: null, not_applicable: String(e) }))));
    setPlans(Object.fromEntries(ids.map((id, i) => [id, got[i]])));
  }, []);

  useEffect(() => {
    if (!running) return;
    load().catch((e) => setMessage({ text: String(e), error: true }));
    api.games().then(setGames, () => setGames([]));
  }, [running, load]);

  if (!running) return <ServiceGate data={data} />;

  const run = async (label: string, f: () => Promise<string | void>) => {
    setBusy(label);
    setMessage(null);
    try {
      const text = await f();
      await load();
      if (text) setMessage({ text });
    } catch (e) {
      setMessage({ text: String(e).replace(/^Error: /, ""), error: true });
    } finally {
      setBusy(null);
    }
  };

  const apply = (id: string, params: Record<string, unknown>, label: string, proceedWithout = false) =>
    run(id + JSON.stringify(params), async () => {
      const r = await changes.apply(id, params, proceedWithout);
      if (r.status === "needs_consent") {
        setConsent({ reason: r.reason, id, params, label });
        return;
      }
      setConsent(null);
      return `${label}: done. You can put it back below at any time.`;
    });

  const groups: Group[] = rows ? group(rows, catalog) : [];
  const open = groups.reduce((n, g) => n + g.open, 0);
  const undoAll = () =>
    run("undo-all", async () => {
      await changes.undoAll();
      data.refreshSession();
      return "Everything Verity changed has been put back.";
    });
  const undoGroup = (g: Group) =>
    run(`undo-${g.txn}`, async () => {
      for (const r of [...g.rows].reverse()) if (r.state === "applied" || r.state === "pending") await changes.revert(r.id);
      return "Put back.";
    });

  const enabledStartup = startup?.filter((s) => s.enabled) ?? [];
  const hybridGpus = (data.info?.gpus.length ?? 0) > 1;

  return (
    <>
      <section className="verdict" aria-labelledby="ch">
        <div>
          <h1 id="ch" aria-live="polite">
            {rows === null ? (
              "Reading Verity's change log…"
            ) : open > 0 ? (
              <>
                <span className="hl-look">{open}</span> {open === 1 ? "change" : "changes"} by Verity in effect now.
              </>
            ) : (
              <>Nothing Verity changed is in effect.</>
            )}
          </h1>
          <p>
            Verity writes down every change before making it, so each one can be put back. Changes made for a game
            session go back on their own when the game closes. If you changed something yourself afterwards, your
            setting is kept.
          </p>
        </div>
        <div className="actions">
          <button className="btn danger" onClick={undoAll} disabled={busy !== null || open === 0}>
            <Icon name="undo" />
            {busy === "undo-all" ? "Putting back…" : "Undo everything"}
          </button>
        </div>
      </section>

      {consent && (
        <section className="banner" aria-labelledby="consent">
          <div>
            <h2 id="consent">No restore point for this first change</h2>
            <p>
              {consent.reason} Verity's own undo still works. Make the change anyway?
            </p>
          </div>
          <div className="actions">
            <button
              className="btn primary"
              onClick={() => apply(consent.id, consent.params, consent.label, true)}
              disabled={busy !== null}
            >
              Make the change
            </button>
            <button className="btn" onClick={() => setConsent(null)} disabled={busy !== null}>
              Cancel
            </button>
          </div>
        </section>
      )}
      {message && (
        <p className={message.error ? "error-text" : "hint"} role="status">
          {message.text}
        </p>
      )}

      <section className="chg" aria-labelledby="fixes">
        <h2 id="fixes" className="section-title">
          Settings Verity can change for you
        </h2>
        <div className="sheet">
          <Fix
            title="Game Mode"
            what="Windows holds back updates and background work while a game runs."
            plan={plans["gamemode.enable"]}
            now="Off now"
            button="Turn on"
            busy={busy}
            onApply={() => apply("gamemode.enable", {}, "Game Mode")}
          />
          <Fix
            title="Refresh rate"
            what="Windows often leaves fast screens at 60 Hz. The screen blinks once while it switches."
            plan={plans["display.max_refresh"]}
            button="Use the highest"
            busy={busy}
            onApply={() => apply("display.max_refresh", {}, "Refresh rate")}
          />
          <div className="chg-row">
            <div>
              <div className="chg-name">Programs that start with Windows</div>
              <div className="chg-what">
                Turned off the same way Task Manager does it; nothing is uninstalled.
              </div>
            </div>
            <div className="chg-list">
              {startup === null && <span className="hint">Reading…</span>}
              {startup && enabledStartup.length === 0 && <span className="chg-state">None left to turn off.</span>}
              {enabledStartup.map((s) => (
                <div className="chg-item" key={s.place + s.name}>
                  <span>{itemName(s.name)}</span>
                  <button
                    className="link"
                    disabled={busy !== null}
                    onClick={() => apply("startup.disable_item", { name: s.name, place: s.place }, itemName(s.name))}
                    aria-label={`Stop ${itemName(s.name)} starting with Windows`}
                  >
                    Turn off
                  </button>
                </div>
              ))}
              {startup && startup.length > enabledStartup.length && (
                <span className="hint">{startup.length - enabledStartup.length} already off.</span>
              )}
            </div>
          </div>
          {hybridGpus && games.length > 0 && (
            <div className="chg-row">
              <div>
                <div className="chg-name">Games on the fast graphics chip</div>
                <div className="chg-what">
                  This PC has two graphics chips; a game can end up on the slower built-in one.
                </div>
              </div>
              <div className="chg-list">
                {games.map((g) => (
                  <div className="chg-item" key={g.id}>
                    <span>{g.name}</span>
                    <button
                      className="link"
                      disabled={busy !== null}
                      onClick={() => apply("gpu.preference", { exe: g.exe_path }, g.name)}
                      aria-label={`Run ${g.name} on the high-performance GPU`}
                    >
                      Use fast GPU
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </section>

      <section className="chg" aria-labelledby="hist">
        <h2 id="hist" className="section-title">
          What Verity changed
        </h2>
        <div className="sheet">
          <div className="chg-hist head colhead" aria-hidden="true">
            <span>When</span>
            <span>Change</span>
            <span>Now</span>
            <span />
          </div>
          {rows !== null && groups.length === 0 && (
            <p className="grow-msg">No changes yet. Session Mode's changes appear here after your next game.</p>
          )}
          {groups.map((g) => (
            <div className="chg-hist" key={g.txn}>
              <span className="chg-when">
                {when(g.started)}
                <span className="hint">{g.source}</span>
              </span>
              <span className="chg-sum">
                {g.summary}
                {g.rows.some((r) => r.state === "superseded") && (
                  <span className="hint chg-note">
                    Kept as you set it: {g.rows.filter((r) => r.state === "superseded").map(rowName).join(", ")}.
                  </span>
                )}
                {g.rows
                  .filter((r) => r.state === "revert_failed" && r.error)
                  .map((r) => (
                    <span className="error-text chg-note" key={r.id}>
                      {r.error}
                    </span>
                  ))}
              </span>
              <span className={`chg-phase ${g.phase}`}>
                <span className={`dot ${g.phase === "on" ? "look" : g.phase === "failed" ? "fix" : "ok"}`} />
                {phaseLabel[g.phase]}
              </span>
              {g.open > 0 ? (
                <button className="btn" onClick={() => undoGroup(g)} disabled={busy !== null}>
                  {busy === `undo-${g.txn}` ? "Putting back…" : "Put back"}
                </button>
              ) : (
                <span />
              )}
            </div>
          ))}
        </div>
      </section>
    </>
  );
}

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function rowName(r: JournalRow): string {
  const p = r.params as { name?: string; exe?: string };
  return p.name ?? (p.exe ? fileName(p.exe) : r.action_id);
}

function Fix(props: {
  title: string;
  what: string;
  plan: Plan | undefined;
  /** What to show instead of the exact change, which stays in the tooltip. */
  now?: string;
  button: string;
  busy: string | null;
  onApply: () => void;
}) {
  const { plan } = props;
  return (
    <div className="chg-row">
      <div>
        <div className="chg-name">{props.title}</div>
        <div className="chg-what">{props.what}</div>
      </div>
      <div className="chg-act">
        {!plan && <span className="hint">Checking…</span>}
        {plan?.change === null && (
          <span className="chg-state">
            <span className="dot ok" />
            {sentence(plan.not_applicable ?? "")}
          </span>
        )}
        {plan?.change && (
          <>
            <span className="chg-plan" title={plan.change}>
              {props.now ?? friendly(plan.change)}
            </span>
            <button className="btn" onClick={props.onApply} disabled={props.busy !== null}>
              {props.button}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
