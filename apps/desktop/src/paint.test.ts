import { describe, expect, it } from "vitest";

import { afterNextPaint, type PaintClock } from "./paint";

describe("afterNextPaint", () => {
  it("reads the clock in a task queued from the frame callback", () => {
    const order: string[] = [];
    const frames: Array<() => void> = [];
    const tasks: Array<() => void> = [];
    let time = 1000;
    const clock: PaintClock = {
      frame: (cb) => {
        order.push("frame requested");
        frames.push(cb);
      },
      later: (cb) => {
        order.push("task queued");
        tasks.push(cb);
      },
      now: () => time,
    };
    const reports: number[] = [];
    afterNextPaint((at) => reports.push(at), clock);
    expect(reports).toEqual([]);
    time = 1100;
    frames.shift()?.();
    expect(reports).toEqual([]);
    time = 1116.5; // the frame was painted
    tasks.shift()?.();
    expect(reports).toEqual([1116.5]);
    expect(order).toEqual(["frame requested", "task queued"]);
  });
});
