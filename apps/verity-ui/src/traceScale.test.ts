import { test } from "node:test";
import assert from "node:assert/strict";
import { duration, ticksFor, yScale } from "./traceScale.ts";

test("one huge frame does not set the scale", () => {
  // Session 12's shape: steady 14 ms frames, a few 60 ms stutters, one 17.8 s load.
  const ms = [...Array(10_000).fill(14), ...Array(30).fill(60), 17_797.8];
  const { top, worst } = yScale(ms);
  assert.equal(worst, 17_797.8);
  assert.ok(top >= 60 && top <= 250, `top ${top}`);
});

test("a slow game keeps its whole line on the chart", () => {
  const { top } = yScale(Array(1000).fill(200));
  assert.ok(top >= 200, `top ${top}`);
  assert.deepEqual(yScale([]), { top: 40, worst: 0 });
  assert.equal(yScale([null, 10]).top, 40);
});

test("axis labels never crowd each other", () => {
  for (const [top, h] of [[40, 200], [69, 200], [250, 200], [250, 60], [400, 120]]) {
    const ticks = ticksFor(top, h);
    assert.ok(ticks.length >= 1);
    assert.ok(ticks.every((t) => t < top));
    for (let i = 1; i < ticks.length; i++) {
      assert.ok(((ticks[i] - ticks[i - 1]) / top) * h >= 16, `${ticks} at top ${top}, height ${h}`);
    }
  }
});

test("durations read naturally", () => {
  assert.equal(duration(17_797.8), "17.8 s");
  assert.equal(duration(212.4), "212 ms");
});
