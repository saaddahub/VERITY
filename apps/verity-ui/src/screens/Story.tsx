import { useState } from "react";
import { api, type Cause, type StoryData } from "../api.ts";
import { Icon } from "../ui.tsx";

/** Plain words for each kind of system event. */
export const kindLabel: Record<string, string> = {
  dpc: "Driver work (DPC)",
  isr: "Driver interrupt",
  hard_fault: "Memory read back from disk",
  disk_io: "Disk activity",
  cpu_burst: "Background CPU use",
  proc_start: "Program started",
  throttle: "CPU slowed by power or heat",
  power_source: "Charger plugged or unplugged",
  scan: "Defender scan",
  update: "Windows Update",
  task: "Scheduled task",
  gpu_other: "Another app on the GPU",
  foreground: "Focus changed",
};

function remedy(c: Cause): string {
  const id = c.remedy.id;
  if (id === "background.process") return `Close ${c.subject} before playing, or keep it from running during games.`;
  if (id === "background.gpu") return `${c.subject} competes for the GPU. Close it or pause what it is doing.`;
  if (id.startsWith("driver.")) {
    const what: Record<string, string> = {
      network: "network adapter",
      gpu: "graphics",
      storage: "storage",
      usb: "USB",
      audio: "audio",
      acpi: "power management",
      other: "device",
    };
    return `Update the ${what[id.slice(7)] ?? "device"} driver (${c.subject}) from the laptop maker or the chip maker.`;
  }
  if (id === "schedule.defender") return "Let Defender scan when you are not playing, for example by scheduling it overnight.";
  if (id === "schedule.windows_update") return "Pause Windows Update while you play, or set active hours.";
  if (id === "schedule.task") return `Move the task ${c.subject} to a time you are not playing.`;
  if (id === "D013") return "The CPU was held back by power or heat. Plug in, raise the laptop, or use the maker's performance mode.";
  if (id === "D021") return "The game ran short of memory and read it back from disk. Close other apps while playing.";
  if (id === "power.charger") return "Keep the charger connected while you play.";
  if (id === "focus") return "Switching windows during play lined up with stutters.";
  return "";
}

const pct = (x: number) => `${Math.round(x * 100)}%`;

export function StoryPanel({ data, error }: { data: StoryData | null; error: string | null }) {
  const [saved, setSaved] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const save = async () => {
    if (!data) return;
    setSaving(true);
    try {
      setSaved(`Saved to ${await api.storyExport(data.session_id)}`);
    } catch (e) {
      setSaved(`Couldn't save the report: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const s = data?.story;
  const u = s?.unexplained;
  const inGame = [
    u && u.in_game_cpu > 0 && {
      n: u.in_game_cpu,
      text: "started inside the game on the CPU side, often shader compiling or loading. Changing Windows settings will not remove these.",
    },
    u && u.in_game_gpu > 0 && {
      n: u.in_game_gpu,
      text: "came from the GPU taking longer on those frames, likely a heavy scene. Lower graphics settings can help.",
    },
    u && u.unknown > 0 && {
      n: u.unknown,
      text: s?.capture_mode === "light"
        ? "had nothing matching in this quick recording. A Session Mode recording also watches drivers, disk and memory."
        : "had no matching system event.",
    },
  ].filter(Boolean) as { n: number; text: string }[];

  return (
    <section className="panel story" aria-labelledby="fs" aria-busy={!data && !error}>
      <div className="trace-head">
        <h2 id="fs">What the stutters coincide with</h2>
        {data && s && s.hitch_groups > 0 && (
          <button className="btn" onClick={save} disabled={saving}>
            <Icon name="download" />
            {saving ? "Saving…" : "Save report"}
          </button>
        )}
      </div>
      {error && <p className="error-text">Couldn't work out the causes: {error}</p>}
      {!data && !error && <p className="hint">Matching each stutter against what the PC was doing…</p>}
      {s && s.hitch_groups === 0 && <p className="story-none">No stutters to explain in this recording.</p>}
      {s && s.hitch_groups > 0 && s.causes.length === 0 && (
        <p className="story-none">
          Nothing on this PC lined up with the stutters more often than chance would explain.
        </p>
      )}
      {s && s.causes.length > 0 && (
        <ol className="causes">
          {s.causes.map((c) => (
            <li key={c.kind + c.subject} className="cause">
              <span className={`clabel ${c.label}`}>{c.label === "likely" ? "Likely" : "Possible"}</span>
              <span className="cwhat">
                <span className="csub reading">{c.subject}</span>
                <span className="ckind">{kindLabel[c.kind] ?? c.kind}</span>
              </span>
              <span className="cnum reading">
                <span>
                  {c.k} of {c.n} stutters
                </span>
                <span>{c.lift.toFixed(1)}× chance</span>
                <span>{pct(c.excess_share)} of stutter time</span>
              </span>
              <span className="cfix">{remedy(c)}</span>
            </li>
          ))}
        </ol>
      )}
      {inGame.length > 0 && (
        <ul className="ingame">
          {inGame.map((x) => (
            <li key={x.text}>
              <span className="reading">{x.n}</span> {x.n === 1 ? "stutter" : "stutters"} {x.text}
            </li>
          ))}
        </ul>
      )}
      {s && s.hitch_groups > 0 && (
        <p className="hint">
          "Coincides with" means it was running while the slow frame was made, more often than chance. Likely means at
          least 3 stutters, 3× chance, and under a 1% fluke after allowing for the {s.pairs_tested} things checked.
          {s.caveats.length > 0 && <> {s.caveats.join(". ")}.</>}
        </p>
      )}
      {saved && (
        <p className="hint" role="status">
          {saved}
        </p>
      )}
    </section>
  );
}
