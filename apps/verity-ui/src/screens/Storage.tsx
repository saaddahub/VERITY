import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { animate, stagger } from "animejs";
import {
  storage,
  type DeleteResult,
  type Drive,
  type DupGroup,
  type Folder,
  type Health,
  type ScanProgress,
  type ScanSummary,
  type StorageItem,
  type Suggestion,
  type TypeGroup,
} from "../api.ts";
import type { Data } from "../App.tsx";
import { Icon, Sheet } from "../ui.tsx";
import { age, bytes, healthChecks, letter, pruneNested, selectedBytes, spaceChecks } from "../storage.ts";

type Tab = "clean" | "folders" | "largest" | "types" | "dupes";
const TABS: { id: Tab; label: string }[] = [
  { id: "clean", label: "Clean up" },
  { id: "folders", label: "Folders" },
  { id: "largest", label: "Largest files" },
  { id: "types", label: "File types" },
  { id: "dupes", label: "Duplicates" },
];

const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const errText = (e: unknown) => String(e).replace(/^Error: /, "");
const count = (n: number, one: string) => `${n.toLocaleString()} ${n === 1 ? one : `${one}s`}`;
const parentOf = (p: string) => p.replace(/[\\/][^\\/]+[\\/]?$/, "");

/** Choice of what to delete, by path, with each item's size. */
type Selection = Map<string, number>;

export function Storage({ data }: { data: Data }) {
  const [drives, setDrives] = useState<Drive[] | null>(null);
  const [root, setRoot] = useState<string | null>(null);
  const [health, setHealth] = useState<Health | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  const [summary, setSummary] = useState<ScanSummary | null>(null);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [folder, setFolder] = useState<Folder | null>(null);
  const [rootTop, setRootTop] = useState<StorageItem | null>(null);
  const [largest, setLargest] = useState<StorageItem[] | null>(null);
  const [types, setTypes] = useState<TypeGroup[] | null>(null);
  const [suggestions, setSuggestions] = useState<Suggestion[] | null>(null);
  const [tab, setTab] = useState<Tab>("clean");
  const [sel, setSel] = useState<Selection>(new Map());
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean; kept?: DeleteResult["kept"] } | null>(null);
  const [confirm, setConfirm] = useState<"delete" | "empty" | null>(null);

  const loadDrives = useCallback(async () => {
    const d = await storage.drives();
    setDrives(d);
    setRoot((r) => r ?? (d.find((x) => letter(x.root) === "C") ?? d[0])?.root ?? null);
  }, []);
  const loadHealth = useCallback(() => {
    storage.health().then(
      (h) => {
        setHealth(h);
        setHealthErr(null);
      },
      (e) => setHealthErr(errText(e)),
    );
  }, []);
  const loadSuggestions = useCallback(() => {
    setSuggestions(null);
    storage.suggestions().then(setSuggestions, (e) => setMessage({ text: errText(e), error: true }));
  }, []);

  useEffect(() => {
    loadDrives().catch((e) => setMessage({ text: errText(e), error: true }));
    loadSuggestions();
  }, [loadDrives, loadSuggestions]);
  useEffect(() => {
    if (data.service?.state === "running") loadHealth();
    else if (data.service) setHealthErr("service not running");
  }, [data.service, loadHealth]);
  useEffect(() => {
    let off: (() => void) | undefined;
    void storage.onProgress(setProgress).then((u) => (off = u));
    return () => off?.();
  }, []);

  const openFolder = useCallback(async (id: number) => {
    const f = await storage.children(id);
    setFolder(f);
    if (id === 0) setRootTop(f.items[0] ?? null);
  }, []);
  const loadResults = useCallback(async () => {
    const [f, l, t] = await Promise.all([storage.children(0), storage.largest(), storage.types()]);
    setFolder(f);
    setRootTop(f.items[0] ?? null);
    setLargest(l);
    setTypes(t);
  }, []);

  const drive = drives?.find((d) => d.root === root) ?? null;
  const L = drive ? letter(drive.root) : "";

  const run = async (label: string, f: () => Promise<void>) => {
    setBusy(label);
    setMessage(null);
    try {
      await f();
    } catch (e) {
      setMessage({ text: errText(e), error: true });
    } finally {
      setBusy(null);
    }
  };

  const scan = () =>
    run("scan", async () => {
      if (!root) return;
      setProgress({ dirs: 0, files: 0, bytes: 0, done: false });
      setSummary(null);
      setFolder(null);
      setSel(new Map());
      const s = await storage.scan(root);
      setSummary(s);
      setProgress(null);
      await loadResults();
      setTab("folders");
    }).finally(() => setProgress(null));

  const optimize = () =>
    run("optimize", async () => {
      const r = await storage.optimize(L);
      setMessage({
        text:
          r.result === 0
            ? `Windows optimized ${L}:. It does this weekly on its own; running it now helps after freeing a lot of space.`
            : `Optimize Drives finished with code ${r.result}. Windows Settings, Optimize Drives, shows the details.`,
        error: r.result !== 0,
      });
    });

  const toggle = (path: string, size: number, on: boolean) =>
    setSel((s) => {
      const n = new Map(s);
      if (on) n.set(path, size);
      else n.delete(path);
      return n;
    });

  const afterChange = async () => {
    await loadDrives();
    loadSuggestions();
    if (summary) {
      const [l, t] = await Promise.all([storage.largest(), storage.types()]);
      setLargest(l);
      setTypes(t);
      await openFolder(folder?.id ?? 0).catch(() => openFolder(0));
      if (folder?.id !== 0) await storage.children(0).then((f) => setRootTop(f.items[0] ?? null));
    }
  };

  const doDelete = () =>
    run("delete", async () => {
      setConfirm(null);
      const r = await storage.remove(pruneNested([...sel.keys()]));
      setSel(new Map());
      const gone = r.deleted.length;
      setMessage({
        text:
          gone === 0
            ? "Nothing was removed."
            : `Removed ${gone} ${gone === 1 ? "item" : "items"}${r.freed ? `, freeing ${bytes(r.freed)}` : ""}. They are in the Recycle Bin unless Windows asked you to delete something for good.`,
        kept: r.kept,
      });
      await afterChange();
    });

  const doEmpty = () =>
    run("empty", async () => {
      setConfirm(null);
      await storage.emptyRecycleBin();
      setMessage({ text: "The Recycle Bin is empty." });
      await afterChange();
    });

  const recycle = suggestions?.find((s) => s.kind === "empty_recycle_bin") ?? null;
  const scanning = busy === "scan";
  const service = data.service?.state === "running";

  let heading: React.ReactNode = "Reading your drives…";
  let lede = "See what fills each drive, then tidy up safely. What you remove goes to the Recycle Bin first.";
  let meta = "";
  if (scanning) {
    heading = <>Scanning {L}:…</>;
    meta = progress ? `${progress.files.toLocaleString()} files · ${bytes(progress.bytes)} counted so far` : "";
    lede = "This reads folder listings only, never the files themselves.";
  } else if (summary && rootTop) {
    heading = (
      <>
        On {letter(summary.root)}:, {rootTop.name} takes the most space: <span className="nowrap">{bytes(rootTop.bytes)}</span>.
      </>
    );
    lede = `Verity counted ${bytes(summary.counted)} in ${summary.files.toLocaleString()} files${
      summary.not_visible > 0 ? `. Another ${bytes(summary.not_visible)} is in places your account cannot open, such as restore points and other accounts` : ""
    }.`;
    meta = summary.cancelled ? "The scan was stopped early, so these numbers are partial." : `Scanned in ${summary.seconds.toFixed(0)} s`;
  } else if (drive) {
    const share = drive.total ? drive.free / drive.total : 1;
    heading = (
      <>
        {L}: has <span className={share < 0.05 ? "hl-fix" : share < 0.1 ? "hl-look" : undefined}>{bytes(drive.free)}</span> free
        of {bytes(drive.total)}.
      </>
    );
  }

  return (
    <>
      <section className="verdict" aria-labelledby="sth">
        <div>
          <h1 id="sth" aria-live="polite">
            {heading}
          </h1>
          <p>{lede}</p>
          {meta && <div className="meta tnum">{meta}</div>}
        </div>
        <div className="actions">
          {drives && drives.length > 1 && (
            <div className="drive-pick" role="radiogroup" aria-label="Drive">
              {drives.map((d) => (
                <label key={d.root}>
                  <input
                    type="radio"
                    name="drive"
                    checked={d.root === root}
                    disabled={busy !== null}
                    onChange={() => {
                      setRoot(d.root);
                      setSummary(null);
                      setFolder(null);
                      setLargest(null);
                      setTypes(null);
                      setSel(new Map());
                    }}
                  />
                  {letter(d.root)}:
                </label>
              ))}
            </div>
          )}
          {scanning ? (
            <button className="btn" onClick={() => void storage.cancel()}>
              <Icon name="stop" />
              Stop scan
            </button>
          ) : (
            <button className="btn primary" onClick={scan} disabled={busy !== null || !root}>
              <Icon name="storage" />
              {summary ? `Scan ${L}: again` : `Scan ${L}:`}
            </button>
          )}
          <button
            className="btn"
            onClick={optimize}
            disabled={busy !== null || !service || !L}
            title={service ? undefined : "Needs the Verity service"}
          >
            <Icon name="refresh" />
            {busy === "optimize" ? "Optimizing…" : `Optimize ${L}:`}
          </button>
        </div>
      </section>

      {message && (
        <div className={message.error ? "error-text" : "hint"} role="status">
          <p className="msg">{message.text}</p>
          {message.kept && message.kept.length > 0 && (
            <ul className="kept">
              {message.kept.map((k) => (
                <li key={k.path}>
                  <span className="reading">{k.path}</span>: {k.reason}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {drives && (
        <Sheet rows={[...spaceChecks(drives), ...healthChecks(health, healthErr)]} foot="Checked just now" />
      )}

      <section className="store" aria-label="What takes the space">
        <Tabs tab={tab} setTab={setTab} />
        <div className="store-body" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
          {tab === "clean" && (
            <Cleanup
              items={suggestions}
              sel={sel}
              toggle={toggle}
              busy={busy !== null}
              onEmpty={() => setConfirm("empty")}
              onStorageSense={() => void storage.openStorageSense()}
            />
          )}
          {tab !== "clean" && !summary && (
            <div className="store-empty">
              <p>{scanning ? "Scanning…" : `Scan ${L}: to see what takes the space.`}</p>
              {!scanning && (
                <button className="btn" onClick={scan} disabled={busy !== null || !root}>
                  Scan {L}:
                </button>
              )}
            </div>
          )}
          {tab === "folders" && summary && folder && (
            <FolderView folder={folder} summary={summary} sel={sel} toggle={toggle} open={(id) => void openFolder(id)} />
          )}
          {tab === "largest" && summary && largest && <LargestView items={largest} sel={sel} toggle={toggle} />}
          {tab === "types" && summary && types && <TypesView groups={types} />}
          {tab === "dupes" && summary && <DupesView sel={sel} toggle={toggle} />}
        </div>
      </section>

      <p className="hint honest">
        Freeing space makes room. It does not make games faster unless a drive is almost full.
      </p>

      {sel.size > 0 && (
        <div className="selbar" role="region" aria-label="Chosen items">
          <span>
            <b className="reading">{pruneNested([...sel.keys()]).length}</b> chosen ·{" "}
            <b className="reading">{bytes(selectedBytes(sel))}</b>
          </span>
          <div className="actions">
            <button className="btn" onClick={() => setSel(new Map())} disabled={busy !== null}>
              Clear
            </button>
            <button className="btn danger-solid" onClick={() => setConfirm("delete")} disabled={busy !== null}>
              Move to Recycle Bin…
            </button>
          </div>
        </div>
      )}

      {confirm === "delete" && (
        <Confirm
          title={`Move ${count(pruneNested([...sel.keys()]).length, "item")} to the Recycle Bin?`}
          body={`You can put ${pruneNested([...sel.keys()]).length === 1 ? "it" : "them"} back from the Recycle Bin. If something is too big for the Recycle Bin, Windows asks before deleting it for good.`}
          action="Move to Recycle Bin"
          onCancel={() => setConfirm(null)}
          onConfirm={doDelete}
        >
          <ul className="confirm-list">
            {pruneNested([...sel.keys()]).map((p) => (
              <li key={p}>
                <span className="reading path">{p}</span>
                <span className="reading">{bytes(sel.get(p) ?? 0)}</span>
              </li>
            ))}
          </ul>
        </Confirm>
      )}
      {confirm === "empty" && recycle && (
        <Confirm
          title="Empty the Recycle Bin?"
          body={`${bytes(recycle.bytes ?? 0)} of deleted items. This removes them for good; it cannot be undone.`}
          action="Empty it"
          onCancel={() => setConfirm(null)}
          onConfirm={doEmpty}
        />
      )}
    </>
  );
}

function Tabs({ tab, setTab }: { tab: Tab; setTab: (t: Tab) => void }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const go = (i: number) => {
    setTab(TABS[i].id);
    refs.current[i]?.focus();
  };
  const move = (dir: number) => go((TABS.findIndex((t) => t.id === tab) + dir + TABS.length) % TABS.length);
  return (
    <div
      className="tabs"
      role="tablist"
      aria-label="Storage views"
      onKeyDown={(e) => {
        if (e.key === "ArrowRight") move(1);
        if (e.key === "ArrowLeft") move(-1);
        if (e.key === "Home") go(0);
        if (e.key === "End") go(TABS.length - 1);
      }}
    >
      {TABS.map((t, i) => (
        <button
          key={t.id}
          ref={(el) => {
            refs.current[i] = el;
          }}
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={t.id === tab}
          aria-controls={`panel-${t.id}`}
          tabIndex={t.id === tab ? 0 : -1}
          className="tab"
          onClick={() => setTab(t.id)}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

function Tick({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean }) {
  return (
    <input
      type="checkbox"
      className="tick"
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
}

/** Size bars grow in once per folder: the screen's one moving moment. */
function useBars(key: string) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (!ref.current || reduced()) return;
    const bars = ref.current.querySelectorAll(".sbar > i");
    const a = animate(bars, { scaleX: { from: 0 }, duration: 560, delay: stagger(18, { start: 60 }), ease: "out(4)" });
    return () => {
      a.revert();
    };
  }, [key]);
  return ref;
}

function ItemRow({
  item,
  share,
  sel,
  toggle,
  open,
  sub,
}: {
  item: StorageItem;
  share: number;
  sel: Selection;
  toggle: (p: string, b: number, on: boolean) => void;
  open?: (id: number) => void;
  sub?: string;
}) {
  const canOpen = open && item.is_dir && !item.link && !item.unreadable;
  const notes = [
    item.unreadable && "your account can't open it",
    item.link && "link to another folder",
    item.online_only && "online only",
  ].filter(Boolean);
  // An unreadable folder's size and age are unknown, not zero.
  const detail =
    sub ??
    (item.unreadable
      ? notes.join(" · ")
      : [item.is_dir ? `${item.files.toLocaleString()} files` : "file", item.newest ? `changed ${age(item.newest)}` : "", ...notes]
          .filter(Boolean)
          .join(" · "));
  return (
    <div className="srow2">
      <span className="tick-cell">
        {item.deletable && !item.unreadable && (
          <Tick
            label={`Choose ${item.name}`}
            checked={sel.has(item.path)}
            onChange={(on) => toggle(item.path, item.bytes, on)}
          />
        )}
      </span>
      <span className="iname">
        {canOpen ? (
          <button className="link folder" onClick={() => open(item.id)}>
            {item.name}
          </button>
        ) : (
          <span className="plain">{item.name}</span>
        )}
        <span className="isub">{detail}</span>
      </span>
      <span className="sbar" aria-hidden="true">
        <i style={{ "--w": `${Math.max(share * 100, 0.6)}%` } as React.CSSProperties} />
      </span>
      <span className="isize reading">
        {item.unreadable ? "unknown" : bytes(item.bytes)}
      </span>
      <button className="link reveal" onClick={() => void storage.reveal(item.path)} aria-label={`Show ${item.name} in File Explorer`}>
        Show
      </button>
    </div>
  );
}

function FolderView({
  folder,
  summary,
  sel,
  toggle,
  open,
}: {
  folder: Folder;
  summary: ScanSummary;
  sel: Selection;
  toggle: (p: string, b: number, on: boolean) => void;
  open: (id: number) => void;
}) {
  const ref = useBars(`f${folder.id}`);
  const atRoot = folder.id === 0;
  const scale = Math.max(folder.bytes + (atRoot ? summary.not_visible : 0), 1);
  return (
    <div ref={ref}>
      <nav className="crumbs" aria-label="Folder path">
        {folder.crumbs.map((c, i) => (
          <span key={c.id}>
            {i > 0 && <span className="sep" aria-hidden="true">›</span>}
            {i === folder.crumbs.length - 1 ? (
              <b aria-current="location">{c.name}</b>
            ) : (
              <button className="link" onClick={() => open(c.id)}>
                {c.name}
              </button>
            )}
          </span>
        ))}
        <span className="crumb-size reading">
          {bytes(folder.bytes)} · {folder.files.toLocaleString()} files
        </span>
      </nav>
      <div className="slist2">
        {folder.items.map((it) => (
          <ItemRow key={it.id} item={it} share={it.bytes / scale} sel={sel} toggle={toggle} open={open} />
        ))}
        {atRoot && summary.not_visible > 0 && (
          <div className="srow2 hidden-row">
            <span className="tick-cell" />
            <span className="iname">
              <span className="plain">Not visible to your account</span>
              <span className="isub">Restore points, other accounts and folders Windows keeps private</span>
            </span>
            <span className="sbar dim" aria-hidden="true">
              <i style={{ "--w": `${(summary.not_visible / scale) * 100}%` } as React.CSSProperties} />
            </span>
            <span className="isize reading">{bytes(summary.not_visible)}</span>
            <span />
          </div>
        )}
        {folder.items.length === 0 && <p className="store-msg">This folder is empty.</p>}
        {folder.more.count > 0 && (
          <p className="store-msg">
            And {folder.more.count.toLocaleString()} smaller items, {bytes(folder.more.bytes)} together.
          </p>
        )}
      </div>
    </div>
  );
}

function LargestView({ items, sel, toggle }: { items: StorageItem[]; sel: Selection; toggle: (p: string, b: number, on: boolean) => void }) {
  const ref = useBars("largest");
  const top = Math.max(items[0]?.bytes ?? 1, 1);
  if (items.length === 0) return <p className="store-msg">No files over 100 MB on this drive.</p>;
  return (
    <div className="slist2" ref={ref}>
      {items.map((it) => (
        <ItemRow
          key={it.id}
          item={it}
          share={it.bytes / top}
          sel={sel}
          toggle={toggle}
          sub={[parentOf(it.path), it.newest ? `changed ${age(it.newest)}` : ""].filter(Boolean).join(" · ")}
        />
      ))}
    </div>
  );
}

function TypesView({ groups }: { groups: TypeGroup[] }) {
  const ref = useBars("types");
  const total = Math.max(groups.reduce((s, g) => s + g.bytes, 0), 1);
  return (
    <div className="slist2" ref={ref}>
      {groups.map((g) => (
        <div className="srow2 type-row" key={g.group}>
          <span className="tick-cell" />
          <span className="iname">
            <span className="plain">{g.group}</span>
            <span className="isub">{g.files.toLocaleString()} files</span>
          </span>
          <span className="sbar" aria-hidden="true">
            <i style={{ "--w": `${Math.max((g.bytes / total) * 100, 0.6)}%` } as React.CSSProperties} />
          </span>
          <span className="isize reading">{bytes(g.bytes)}</span>
          <span className="share reading">{((g.bytes / total) * 100).toFixed(0)}%</span>
        </div>
      ))}
    </div>
  );
}

function DupesView({ sel, toggle }: { sel: Selection; toggle: (p: string, b: number, on: boolean) => void }) {
  const [groups, setGroups] = useState<DupGroup[] | null>(null);
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    void storage.onDupesProgress(setProg).then((u) => (off = u));
    return () => off?.();
  }, []);
  const find = async () => {
    setRunning(true);
    setError(null);
    setProg(null);
    try {
      setGroups(await storage.duplicates());
    } catch (e) {
      setError(errText(e));
    } finally {
      setRunning(false);
    }
  };
  if (!groups) {
    return (
      <div className="store-empty">
        <p>
          {running
            ? prog
              ? `Compared ${prog.done.toLocaleString()} of ${prog.total.toLocaleString()} files…`
              : "Looking for files of the same size…"
            : "Finds files with exactly the same contents. Verity reads every file over 1 MB that shares its size with another, so this takes a while on a big drive."}
        </p>
        {error && <p className="error-text">{error}</p>}
        {running ? (
          <button className="btn" onClick={() => void storage.cancel()}>
            <Icon name="stop" />
            Stop
          </button>
        ) : (
          <button className="btn" onClick={() => void find()}>
            Find duplicates
          </button>
        )}
      </div>
    );
  }
  if (groups.length === 0) return <p className="store-msg">No duplicate files over 1 MB.</p>;
  return (
    <div className="dupes">
      <p className="store-msg">
        {groups.length} sets of identical files · {bytes(groups.reduce((s, g) => s + g.wasted, 0))} to gain by keeping one of
        each. The newest copy is listed first.
      </p>
      {groups.map((g) => (
        <div className="dgroup" key={g.items[0]?.path}>
          <div className="dhead colhead">
            {g.items.length} copies · {bytes(g.size)} each
          </div>
          {g.items.map((it, i) => (
            <ItemRow
              key={it.id}
              item={it}
              share={0}
              sel={sel}
              toggle={toggle}
              sub={[i === 0 ? "newest" : "", parentOf(it.path)].filter(Boolean).join(" · ")}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

function Cleanup({
  items,
  sel,
  toggle,
  busy,
  onEmpty,
  onStorageSense,
}: {
  items: Suggestion[] | null;
  sel: Selection;
  toggle: (p: string, b: number, on: boolean) => void;
  busy: boolean;
  onEmpty: () => void;
  onStorageSense: () => void;
}) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  if (!items) return <p className="store-msg">Looking for things you can clear…</p>;
  if (items.length === 0) return <p className="store-msg">Nothing to clear right now.</p>;
  return (
    <div className="clean">
      {items.map((s) => {
        const all = s.items.length > 0 && s.items.every((i) => sel.has(i.path));
        const some = s.items.some((i) => sel.has(i.path));
        return (
          <div className="crow" key={s.id}>
            <span className="tick-cell">
              {s.kind === "delete" && (
                <input
                  type="checkbox"
                  className="tick"
                  aria-label={`Choose all ${s.title.toLowerCase()}`}
                  checked={all}
                  ref={(el) => {
                    if (el) el.indeterminate = some && !all;
                  }}
                  onChange={(e) => s.items.forEach((i) => toggle(i.path, i.bytes, e.target.checked))}
                  disabled={busy}
                />
              )}
            </span>
            <div className="ctext">
              <b>{s.title}</b>
              <span>{s.detail}</span>
              {s.kind === "delete" && (
                <button className="link" onClick={() => setOpen((o) => ({ ...o, [s.id]: !o[s.id] }))} aria-expanded={!!open[s.id]}>
                  {open[s.id] ? "Hide" : "Show"} {s.items.length} {s.items.length === 1 ? "item" : "items"}
                </button>
              )}
            </div>
            {s.bytes === null ? (
              <span className="isize unknown">Windows can tell you</span>
            ) : (
              <span className="isize reading">{bytes(s.bytes)}</span>
            )}
            <span className="cact">
              {s.kind === "empty_recycle_bin" && (
                <button className="btn danger" onClick={onEmpty} disabled={busy}>
                  Empty Recycle Bin…
                </button>
              )}
              {s.kind === "open_storage_sense" && (
                <button className="btn" onClick={onStorageSense}>
                  Open Storage Sense
                </button>
              )}
            </span>
            {s.kind === "delete" && open[s.id] && (
              <ul className="citems">
                {s.items.map((i) => (
                  <li key={i.path}>
                    <Tick label={`Choose ${i.path}`} checked={sel.has(i.path)} onChange={(on) => toggle(i.path, i.bytes, on)} disabled={busy} />
                    <span className="reading path">{i.path}</span>
                    <span className="isub">{age(i.modified)}</span>
                    <span className="reading">{bytes(i.bytes)}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** A confirmation that holds focus until answered; Escape cancels. */
function Confirm({
  title,
  body,
  action,
  onCancel,
  onConfirm,
  children,
}: {
  title: string;
  body: string;
  action: string;
  onCancel: () => void;
  onConfirm: () => void;
  children?: React.ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (d && !d.open) d.showModal();
  }, []);
  return (
    <dialog ref={ref} className="confirm" aria-labelledby="cft" onCancel={onCancel}>
      <h2 id="cft">{title}</h2>
      <p>{body}</p>
      {children}
      <div className="actions">
        <button className="btn" onClick={onCancel} autoFocus>
          Cancel
        </button>
        <button className="btn danger-solid" onClick={onConfirm}>
          {action}
        </button>
      </div>
    </dialog>
  );
}
