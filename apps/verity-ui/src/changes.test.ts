import { test } from "node:test";
import assert from "node:assert/strict";
import type { ActionInfo, JournalRow } from "./api.ts";
import { friendly, group, itemName, phase, when } from "./changes.ts";

const row = (id: number, txn: string, action_id: string, state: JournalRow["state"], params = {}): JournalRow => ({
  id,
  txn_id: txn,
  action_id,
  scope: "session",
  executor: "service",
  params,
  snapshot: {},
  owner: "",
  state,
  created_utc: "2026-11-26T10:00:00Z",
  applied_utc: null,
  reverted_utc: null,
  error: null,
});

const catalog = [{ id: "gamemode.enable", title: "Turn on Game Mode" }] as ActionInfo[];

test("a session is one group, summarised in plain words", () => {
  const rows = [
    row(1, "session-a", "power.session_overlay", "reverted"),
    row(2, "session-a", "sched.background_ecoqos", "reverted", { name: "chrome.exe" }),
    row(3, "session-a", "sched.background_ecoqos", "superseded", { name: "app.exe" }),
    row(4, "app-1", "gamemode.enable", "applied"),
  ];
  const g = group(rows, catalog);
  assert.deepEqual(
    g.map((x) => x.txn),
    ["app-1", "session-a"],
  );
  assert.equal(g[0].source, "Made in Verity");
  assert.equal(g[0].open, 1);
  assert.equal(g[0].phase, "on");
  assert.equal(g[0].summary, "Turn on Game Mode");
  assert.equal(g[1].summary, "Best performance power mode · Efficiency mode for 2 background apps");
  assert.equal(g[1].phase, "back");
});

test("plan lines and item names read plainly", () => {
  assert.equal(friendly("\\\\.\\DISPLAY1: 60 Hz -> 144 Hz at 1920x1080"), "60 Hz → 144 Hz at 1920x1080");
  assert.equal(itemName("Ollama.lnk"), "Ollama");
  assert.equal(itemName("Steam"), "Steam");
});

test("states read as people say them", () => {
  assert.equal(phase("pending"), "on");
  assert.equal(phase("revert_failed"), "failed");
  assert.equal(phase("reverted"), "back");
});

test("dates are relative to today", () => {
  const now = new Date("2026-11-26T12:00:00Z");
  assert.match(when("2026-11-26T10:00:00Z", now), /^Today /);
  assert.match(when("2026-11-25T10:00:00Z", now), /^Yesterday /);
  assert.equal(when("bad", now), "bad");
});
