import { test } from "node:test";
import assert from "node:assert/strict";
import { age, bytes, healthChecks, pruneNested, selectedBytes, spaceChecks } from "./storage.ts";
import type { Disk } from "./api.ts";

test("sizes read like Explorer", () => {
  assert.equal(bytes(512), "512 B");
  assert.equal(bytes(1536), "1.5 KB");
  assert.equal(bytes(109_058_310_144), "102 GB");
  assert.equal(bytes(4.5 * 1024 ** 3), "4.5 GB");
});

test("a folder and something inside it are counted once", () => {
  assert.deepEqual(pruneNested(["C:\\A\\b.txt", "C:\\A", "c:\\ab\\x", "C:\\A\\"]), ["C:\\A", "c:\\ab\\x"]);
  const sel = new Map([
    ["C:\\A", 100],
    ["C:\\A\\b.txt", 30],
    ["D:\\z", 5],
  ]);
  assert.equal(selectedBytes(sel), 105);
});

test("free space marks", () => {
  const gb = 1024 ** 3;
  const marks = spaceChecks([
    { root: "C:\\", total: 500 * gb, free: 100 * gb },
    { root: "D:\\", total: 500 * gb, free: 40 * gb },
    { root: "E:\\", total: 500 * gb, free: 10 * gb },
  ]).map((c) => [c.system, c.mark]);
  assert.deepEqual(marks, [["C: space", "ok"], ["D: space", "look"], ["E: space", "fix"]]);
});

test("health marks say why", () => {
  const d: Disk = {
    model: "NVMe Micron_2400_MTFDKBA512QFM", media: "SSD", health: "Healthy", size: 512e9,
    wear: 3, temperature: 41, temperature_max: 84, read_errors: 0, write_errors: 0, power_on_hours: 5321,
  };
  const [ok] = healthChecks({ disks: [d], volumes: [] }, null);
  assert.equal(ok.system, "Micron 2400 SSD");
  assert.equal(ok.reading, "Healthy · 3% worn · 41 °C");
  assert.equal(ok.mark, "ok");
  assert.equal(healthChecks({ disks: [{ ...d, temperature: 82 }], volumes: [] }, null)[0].mark, "look");
  assert.equal(healthChecks({ disks: [{ ...d, read_errors: 2 }], volumes: [] }, null)[0].mark, "fix");
  const bare = healthChecks({ disks: [{ ...d, wear: null, temperature: null, temperature_max: null }], volumes: [] }, null)[0];
  assert.equal(bare.reading, "Healthy");
  assert.match(bare.note, /does not report/);
  assert.equal(healthChecks(null, "pipe closed")[0].mark, "none");
});

test("ages", () => {
  const now = Date.UTC(2026, 10, 24);
  const ft = (ms: number) => (ms + 11_644_473_600_000) * 10_000;
  assert.equal(age(ft(now - 3 * 86_400_000), now), "3 days ago");
  assert.equal(age(ft(now - 200 * 86_400_000), now), "7 months ago");
  assert.equal(age(0, now), "");
});
