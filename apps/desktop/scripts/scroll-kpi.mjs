// Measures how smoothly the snippet list scrolls with 10,000 snippets (specification §10, task
// 3.3's KPI: 60 frames per second). It runs the *built* front end (`pnpm build`, `dist/`) in
// Chromium under Playwright with a mocked command layer: `window.__TAURI_INTERNALS__.invoke`
// answers the `library_*` commands from an in-page library of N snippets, so the real
// bundle, the real `api.ts` and the real list render, and only the service is missing.
//
//   node scripts/scroll-kpi.mjs [--rows 10000] [--seconds 8] [--shot out.png] [--json]
//
// Playwright is not a dependency of the app: point NODE_PATH (or PLAYWRIGHT_MODULE) at an
// install that has it. Frame times come from `requestAnimationFrame` callbacks: the gap between
// two callbacks is the frame time, and a frame longer than 25 ms is a dropped frame (one or
// more 16.7 ms vsyncs missed).
//
// Two drivers: "wheel" sends real wheel events through the browser (the scroll runs on the
// compositor, as a person's trackpad does), "script" moves `scrollTop` from a rAF callback so
// that every frame also costs the main thread a scroll event, a state update and a render of the
// rows that came into view (the worst case, and the one that exercises the list's code).

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "../dist");
const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
};
const rows = Number(arg("rows", "10000"));
const seconds = Number(arg("seconds", "8"));
const shot = arg("shot", "");
const asJson = process.argv.includes("--json");

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? "playwright");

if (!existsSync(join(dist, "index.html"))) {
  console.error("dist/ is missing: run `pnpm build` first");
  process.exit(2);
}

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" };
const server = createServer((req, res) => {
  const path = join(dist, req.url === "/" ? "index.html" : decodeURIComponent((req.url ?? "").split("?")[0]));
  if (!path.startsWith(dist) || !existsSync(path) || !statSync(path).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
  createReadStream(path).pipe(res);
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const url = `http://127.0.0.1:${server.address().port}/`;

import { mock } from "./tauri-mock.mjs";

const percentile = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)];
const summarise = (frames) => {
  const sorted = [...frames].sort((a, b) => a - b);
  const sum = frames.reduce((a, b) => a + b, 0);
  return {
    frames: frames.length,
    seconds: +(sum / 1000).toFixed(2),
    fps: +((frames.length / sum) * 1000).toFixed(1),
    p50_ms: +percentile(sorted, 50).toFixed(2),
    p95_ms: +percentile(sorted, 95).toFixed(2),
    p99_ms: +percentile(sorted, 99).toFixed(2),
    max_ms: +sorted[sorted.length - 1].toFixed(2),
    dropped_over_25ms: frames.filter((f) => f > 25).length,
    dropped_over_42ms: frames.filter((f) => f > 41.7).length,
  };
};

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.addInitScript(mock, rows);
await page.goto(url);
await page.waitForSelector('[role="listbox"] [role="option"]');
await page.waitForFunction((n) => document.querySelector('[role="option"]')?.getAttribute("aria-setsize") === String(n), rows);
const domRows = await page.evaluate(() => document.querySelectorAll('[role="listbox"] [role="option"]').length);
const rowHeight = 48;
if (shot) await page.screenshot({ path: shot });

const record = (driver, perSecond) =>
  page.evaluate(
    ({ driver, perSecond, ms, rowHeight }) =>
      new Promise((done) => {
        const list = document.querySelector('[role="listbox"]');
        const max = list.scrollHeight - list.clientHeight;
        const frames = [];
        let last;
        let pos = 0;
        let dir = 1;
        const end = performance.now() + ms;
        const tick = (t) => {
          if (last !== undefined) frames.push(t - last);
          const dt = last === undefined ? 0 : t - last;
          last = t;
          if (driver === "script") {
            pos += (dir * perSecond * dt) / 1000;
            if (pos > max || pos < 0) {
              dir = -dir;
              pos = Math.max(0, Math.min(max, pos));
            }
            list.scrollTop = pos;
          }
          if (performance.now() < end) requestAnimationFrame(tick);
          else done({ frames, shown: list.querySelectorAll('[role="option"]').length, top: list.scrollTop, rowHeight });
        };
        list.scrollTop = 0;
        requestAnimationFrame(tick);
      }),
    { driver, perSecond, ms: seconds * 1000, rowHeight },
  );

const results = {};
const settle = () => page.waitForTimeout(500);

// Real wheel input, 120 px per notch at 60 notches a second: 7,200 px/s = 150 rows/s.
{
  await page.evaluate(() => (document.querySelector('[role="listbox"]').scrollTop = 0));
  await page.mouse.move(400, 400);
  const pending = record("wheel", 0);
  const stop = Date.now() + seconds * 1000;
  while (Date.now() < stop) {
    await page.mouse.wheel(0, 120);
    await page.waitForTimeout(1000 / 60);
  }
  const r = await pending;
  results.wheel = { ...summarise(r.frames.slice(2)), reachedPx: Math.round(r.top) };
  await settle();
}
// The script driver at three speeds: 6,000 px/s, 24,000 px/s (500 rows a second) and 96,000.
for (const speed of [6000, 24000, 96000]) {
  const r = await record("script", speed);
  results[`script_${speed}px_s`] = { ...summarise(r.frames.slice(2)), rowsInPage: r.shown };
  await settle();
}

// Headroom: the same scroll with the CPU slowed down 4 and 6 times (a laptop on battery, a
// busy machine), through the DevTools protocol.
const cdp = await page.context().newCDPSession(page);
for (const rate of [4, 6]) {
  await cdp.send("Emulation.setCPUThrottlingRate", { rate });
  const r = await record("script", 24000);
  results[`script_24000px_s_cpu_x${rate}`] = { ...summarise(r.frames.slice(2)), rowsInPage: r.shown };
  await settle();
}
await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });

const env = {
  chromium: browser.version(),
  rowsInList: rows,
  rowsInThePage: domRows,
  seconds,
  cpus: (await import("node:os")).cpus().length,
  cpu: (await import("node:os")).cpus()[0]?.model,
};
await browser.close();
server.close();
if (asJson) console.log(JSON.stringify({ env, results }, null, 2));
else {
  console.log("environment:", env);
  console.table(results);
}
