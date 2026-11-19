// The first-paint report of the cold-open measurement (src-tauri/src/timing.rs). A callback of
// `requestAnimationFrame` runs before the frame is painted; a task queued from it runs after,
// so the time read there is the first paint's.

export interface PaintClock {
  frame: (callback: () => void) => void;
  later: (callback: () => void) => void;
  now: () => number;
}

const browserClock: PaintClock = {
  frame: (cb) => {
    requestAnimationFrame(() => cb());
  },
  later: (cb) => {
    setTimeout(cb, 0);
  },
  // Milliseconds since the Unix epoch, like the Rust side's wall clock.
  now: () => performance.timeOrigin + performance.now(),
};

/** Calls `report` once, with the time of the next paint. */
export function afterNextPaint(
  report: (paintedAtMs: number) => void,
  clock: PaintClock = browserClock,
): void {
  clock.frame(() => {
    clock.later(() => {
      report(clock.now());
    });
  });
}

/** When this page's script began to run (epoch milliseconds), read when the module loads. */
export const scriptStartedAtMs: number =
  typeof performance === "undefined" ? 0 : performance.timeOrigin + performance.now();

/** When the page's navigation began (epoch milliseconds). */
export function navigationStartedAtMs(): number {
  return typeof performance === "undefined" ? 0 : performance.timeOrigin;
}
