// The keyboard-only run (task 3.11, docs/ux/spec.md §5 and §8; docs/accessibility-run.md). It
// walks the built front end (`pnpm build`, `dist/`) in headless Chromium under Playwright, over
// the command layer of `keyboard-mock.mjs`, using the keyboard only: no click, no pointer, no
// `focus()` call. After every step it checks that the focus is on something that can be seen and
// has a visible ring (WCAG 2.4.7) and runs axe-core in the real browser (contrast, target size,
// scrolling regions: the rules jsdom cannot judge). It makes the walk once in each of the four
// settings the design must follow: the light and the dark theme, reduced motion, forced colours.
//
//   node scripts/keyboard-run.mjs [--only light] [--shots dir] [--json]
//
// Playwright is not a dependency of the app: point PLAYWRIGHT_MODULE at an install that has it
// (CI installs it in a temporary directory). The browser is the runner's Chrome
// (IAMB_CHROME_CHANNEL, default `chrome`) or the executable IAMB_CHROME_PATH.
//
// What the walk covers, in order: the library (focus the panes, create a snippet, write its
// template with an insert list and a field, make it formatted, Tab out of the editor, save,
// preview, the context menu, a dialog with its focus trap and its return, delete and undo),
// settings (change one), library health and "Why didn't it expand?", the search palette (find
// the snippet and insert it), the fill-in form (answer every kind of field and insert), and the
// onboarding window's first two steps. The exit code is 1 when a step fails.

import { createReadStream, existsSync, mkdirSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { labelInName } from "./label-in-name.mjs";
import { mock } from "./keyboard-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "../dist");
const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
};
const only = arg("only", "");
const shots = arg("shots", "");
const asJson = process.argv.includes("--json");

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const axeSource = join(dirname(require.resolve("axe-core/package.json", { paths: [here] })), "axe.min.js");

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

const MODES = [
  { name: "light", context: { colorScheme: "light" }, contrast: true },
  { name: "dark", context: { colorScheme: "dark" }, contrast: true },
  { name: "reduced-motion", context: { colorScheme: "light", reducedMotion: "reduce" }, contrast: true },
  // In forced colours the browser replaces the colours: axe's contrast rule has nothing to say.
  { name: "forced-colors", context: { colorScheme: "light", forcedColors: "active" }, contrast: false },
];

const launch = () =>
  chromium.launch(
    process.env.IAMB_CHROME_PATH
      ? { executablePath: process.env.IAMB_CHROME_PATH }
      : { channel: process.env.IAMB_CHROME_CHANNEL ?? "chrome" },
  );

/** The failures of every run: `[mode, step, what]`. */
const failures = [];
/** axe findings that are left (moderate or minor), and the counts of every step. */
const results = [];

class Walk {
  constructor(page, mode) {
    this.page = page;
    this.mode = mode;
    this.n = 0;
  }

  fail(step, what) {
    failures.push([this.mode.name, step, what]);
    if (!asJson) console.log(`  FAIL ${step}: ${what}`);
  }

  async key(...keys) {
    for (const k of keys) await this.page.keyboard.press(k);
  }

  async type(text) {
    await this.page.keyboard.type(text);
  }

  /** What has the keyboard: tag, role, name and a short text, for the messages. */
  active() {
    return this.page.evaluate(() => {
      const el = document.activeElement;
      if (!el || el === document.body) return "body";
      const label = el.getAttribute("aria-label") ?? el.id ?? "";
      return `${el.tagName.toLowerCase()}${el.getAttribute("role") ? `[${el.getAttribute("role")}]` : ""} ${label} "${(el.textContent ?? "").trim().slice(0, 30)}"`;
    });
  }

  /** WCAG 2.4.7: something has the focus, it is on screen, and it shows a ring. */
  async focusVisible(step) {
    const r = await this.page.evaluate(() => {
      const el = document.activeElement;
      if (!el || el === document.body || el === document.documentElement) return { ok: false, why: "the focus is on the page, not on a control" };
      const box = el.getBoundingClientRect();
      if (box.width < 1 || box.height < 1) return { ok: false, why: "the focused element has no size" };
      if (box.bottom < 0 || box.right < 0 || box.top > innerHeight || box.left > innerWidth) return { ok: false, why: "the focused element is off screen" };
      const ring = (e) => {
        const s = getComputedStyle(e);
        const outline = s.outlineStyle !== "none" && parseFloat(s.outlineWidth) >= 1;
        const shadow = s.boxShadow !== "none" && /\d/.test(s.boxShadow);
        return outline || shadow;
      };
      if (ring(el)) return { ok: true };
      // A ring on the field around a bare input or on a wrapper that has the focus within.
      for (let e = el.parentElement, i = 0; e && i < 4; e = e.parentElement, i++) {
        if (e.matches(":focus-within") && ring(e)) return { ok: true };
      }
      return { ok: false, why: "no outline or shadow on the focused element or its wrapper" };
    });
    if (!r.ok) this.fail(step, `${r.why}: ${await this.active()}`);
  }

  /** axe-core in the browser; critical and serious fail the step, the rest is listed. */
  async axe(step) {
    if ((await this.page.evaluate(() => typeof window.axe)) === "undefined") await this.page.addScriptTag({ path: axeSource });
    const found = await this.page.evaluate(async (contrast) => {
      const out = await window.axe.run(document, {
        runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] },
        rules: { "color-contrast": { enabled: contrast } },
        resultTypes: ["violations"],
      });
      return out.violations.map((v) => ({ id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.map((n) => `${n.target.join(" ")}`).slice(0, 4), count: v.nodes.length }));
    }, this.mode.contrast);
    const counts = { critical: 0, serious: 0, moderate: 0, minor: 0 };
    for (const f of found) counts[f.impact] += f.count;
    // 2.5.3 Label in name, by the spec's rule (label-in-name.mjs), in the real browser.
    const wrong = await this.page.evaluate(labelInName);
    if (wrong.length) found.push({ id: "label-in-name", impact: "serious", help: "The accessible name starts with the visible label", nodes: wrong, count: wrong.length });
    results.push({ mode: this.mode.name, step, counts, left: found.filter((f) => f.impact === "moderate" || f.impact === "minor") });
    for (const f of found) {
      if (f.impact === "critical" || f.impact === "serious") this.fail(step, `axe ${f.impact} ${f.id} (${f.count}): ${f.help} :: ${f.nodes.join(" | ")}`);
    }
  }

  /** One step done: the focus is visible, axe finds nothing serious, and the optional shot. */
  async check(step, { ring = true } = {}) {
    this.n += 1;
    if (ring) await this.focusVisible(step);
    await this.axe(step);
    if (shots) {
      mkdirSync(shots, { recursive: true });
      await this.page.screenshot({ path: join(shots, `${this.mode.name}-${String(this.n).padStart(2, "0")}-${step.replace(/\W+/g, "-")}.png`) });
    }
    if (!asJson) console.log(`  ok   ${step}`);
  }

  async expect(step, what, ok) {
    if (!(await ok())) this.fail(step, `expected ${what}; the keyboard is on ${await this.active()}`);
  }

  /** Presses Tab (or Shift+Tab) until `fn(arg)` is true of the page; fails after `max` presses. */
  async tabTo(step, what, fn, arg, { max = 40, back = false } = {}) {
    for (let i = 0; i <= max; i++) {
      if (await this.page.evaluate(fn, arg)) return i;
      await this.key(back ? "Shift+Tab" : "Tab");
    }
    this.fail(step, `${what} cannot be reached with ${back ? "Shift+Tab" : "Tab"} (${max} presses); the keyboard is on ${await this.active()}`);
    return -1;
  }

  /** Waits for `fn` in the page to be true. */
  async until(step, what, fn, arg) {
    try {
      await this.page.waitForFunction(fn, arg, { timeout: 4000 });
    } catch {
      this.fail(step, `timed out waiting for ${what}; the keyboard is on ${await this.active()}`);
    }
  }

  /** The text of the live regions (`status`, `alert`) now. */
  live() {
    return this.page.evaluate(() => [...document.querySelectorAll('[role="status"],[role="alert"],[aria-live]')].map((e) => e.textContent.trim()).filter(Boolean));
  }

  activeMatches(selector) {
    return this.page.evaluate((s) => !!document.activeElement?.closest(s), selector);
  }
}

async function open(browser, mode, config, seedUrl = "") {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, ...mode.context });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.addInitScript(mock, config);
  await page.goto(url + seedUrl);
  return { context, page, errors };
}

const SEED = [
  { abbreviation: ";thanks", label: "Thanks for your order", body: "Thank you for your order." },
  { abbreviation: ";late", label: "Late delivery", body: 'Hi {field name="First name"}, sorry about the wait.' },
  { abbreviation: ";hours", label: "Opening hours", body: "We are open Tuesday to Saturday." },
];

async function walkLibrary(browser, mode) {
  const { context, page, errors } = await open(browser, mode, { window: "library", snippets: SEED });
  const w = new Walk(page, mode);
  await page.waitForSelector('[role="listbox"] [role="option"]');
  await w.until("start", "the window's title", () => document.title.startsWith("Iamb"));

  // 1. The panes, by the keyboard map (§5.2).
  await w.key("Control+2");
  await w.expect("panes", "Ctrl+2 to focus the list", () => w.page.evaluate(() => document.activeElement?.getAttribute("role") === "listbox"));
  await w.check("list has the focus");
  await w.key("F6");
  await w.check("F6 moves to the next pane");
  await w.key("Control+1");
  await w.expect("panes", "Ctrl+1 to focus the sidebar", () => w.activeMatches("[data-pane='sidebar']"));
  await w.check("sidebar has the focus");
  await w.key("Control+2");

  // 2. A new snippet: abbreviation, label, then the template editor.
  await w.key("Control+n");
  await w.until("new snippet", "the abbreviation field", () => document.activeElement?.labels?.[0]?.textContent.startsWith("Abbreviation"));
  await w.check("a new snippet starts in Abbreviation");
  await w.type(";greet");
  await w.key("Tab");
  await w.type("Greeting for a customer");
  await w.check("the label is typed");
  // The insert buttons come before the text in the reading order; Tab goes through them to the text.
  const isText = () => page.evaluate(() => document.activeElement?.getAttribute("role") === "textbox" && document.activeElement.isContentEditable);
  let stops = 0;
  do {
    await w.key("Tab");
    stops += 1;
  } while (stops < 20 && !(await isText()));
  await w.check(`Tab reaches the Content field (${stops} stops after Label)`);
  await w.expect("editor", "the Content field after Label", isText);
  await w.type("Hello ");
  await w.type("{");
  await w.until("insert list", "the Insert list", () => !!document.querySelector('[role="listbox"][aria-label="Insert"]'));
  await w.check("the Insert list is open");
  await w.type("fie");
  await w.key("Enter");
  await w.until("insert list", "the list closed", () => !document.querySelector('[role="listbox"][aria-label="Insert"]'));
  await w.check("a field was inserted");
  await w.key("Escape");
  await w.until("card", "the focus back in the editor after Esc", () => document.activeElement?.getAttribute("role") === "textbox" || !!document.activeElement?.closest("[role='dialog'],[role='group'],.tpl-card"));
  await w.check("after the card");

  // The preview has answered for the field (it offers "Try the form", a stop in the tab order
  // right after the editor), so the order does not change between the two key presses below.
  await w.until("preview", "the preview of the field", () => !!document.querySelector(".tpl-fields"));
  // Tab leaves the editor: it never inserts a tab (§5.3, WCAG 2.1.2).
  const before = await page.evaluate(() => document.querySelector('[role="textbox"][contenteditable="true"]')?.textContent ?? "");
  await w.key("Tab");
  const after = await page.evaluate(() => document.querySelector('[role="textbox"][contenteditable="true"]')?.textContent ?? "");
  if (after.includes("\t") || after !== before) w.fail("tab", "Tab changed the editor's text");
  await w.expect("tab", "Tab to move the focus out of the editor", () => page.evaluate(() => document.activeElement?.getAttribute("role") !== "textbox" || !document.activeElement.isContentEditable));
  await w.check("Tab left the editor");
  await w.key("Shift+Tab");
  await w.expect("tab", "Shift+Tab to come back to the editor", () => page.evaluate(() => document.activeElement?.getAttribute("role") === "textbox"));

  // 3. Formatted: the Kind group, reached by Tab, switched with the space bar.
  await w.tabTo("kind", "the Formatted button", () => document.activeElement?.tagName === "BUTTON" && document.activeElement.textContent.trim() === "Formatted");
  await w.check("the Formatted button");
  await w.key("Space");
  await w.until("kind", "the formatting toolbar", () => !!document.querySelector('[role="toolbar"][aria-label="Formatting"]'));
  await w.check("the editor is formatted");
  await w.tabTo("kind", "the Content field again", () => document.activeElement?.getAttribute("role") === "textbox" && document.activeElement.isContentEditable, undefined, { back: true });
  await w.key("Control+End");
  await w.key("Control+b");
  await w.type("Kim Lee");
  await w.key("Control+b");
  await w.check("bold text typed in a formatted body");

  // 4. Save with Ctrl+S; the live region says so (4.1.3).
  await w.key("Control+s");
  await w.until("save", "the snippet saved", () => window.__mock.state.saved.length > 0);
  await w.until("save", "the saved message in a live region", () => [...document.querySelectorAll('[role="status"]')].some((e) => /^Saved/.test(e.textContent.trim())));
  await w.check("saved");
  const saved = await page.evaluate(() => window.__mock.state.saved.at(-1));
  if (!saved || saved.abbreviation !== ";greet" || saved.kind !== "rich" || !/Hello/.test(saved.body) || !/\*\*Kim Lee\*\*/.test(saved.body)) w.fail("save", `the saved snippet is not what was typed: ${JSON.stringify(saved)}`);

  // 5. Preview and back (Ctrl+Shift+P).
  await w.key("Control+Shift+P");
  await w.check("preview");
  await w.key("Control+Shift+P");
  await w.check("back from the preview");

  // 6. The list: filter, context menu from the keyboard, and delete with Undo.
  await w.key("Control+2");
  await w.key("Shift+F10");
  await w.until("menu", "the context menu", () => !!document.querySelector('[role="menu"]'));
  await w.expect("menu", "the focus inside the menu", () => w.activeMatches('[role="menu"]'));
  await w.check("the context menu has the focus");
  await w.key("Escape");
  await w.until("menu", "the menu closed", () => !document.querySelector('[role="menu"]'));
  await w.expect("menu", "the focus back on the list", () => page.evaluate(() => !!document.activeElement?.closest('[role="listbox"]')));
  await w.check("the focus came back to the list");

  // 7. A dialog traps the focus and gives it back (§5.6).
  await w.key("Control+Shift+n");
  await w.until("dialog", "the New folder dialog", () => !!document.querySelector('[role="dialog"]'));
  await w.expect("dialog", "the focus inside the dialog", () => w.activeMatches('[role="dialog"]'));
  await w.check("the dialog has the focus");
  for (let i = 0; i < 8; i++) {
    await w.key("Tab");
    if (!(await w.activeMatches('[role="dialog"]'))) w.fail("dialog", `Tab left the dialog on step ${i + 1}: ${await w.active()}`);
  }
  for (let i = 0; i < 3; i++) {
    await w.key("Shift+Tab");
    if (!(await w.activeMatches('[role="dialog"]'))) w.fail("dialog", `Shift+Tab left the dialog: ${await w.active()}`);
  }
  await w.key("Escape");
  await w.until("dialog", "the dialog closed", () => !document.querySelector('[role="dialog"]'));
  await w.expect("dialog", "the focus back in the window", () => page.evaluate(() => document.activeElement !== document.body));
  await w.check("the focus came back from the dialog");

  await w.key("Control+2");
  await w.key("Delete");
  await w.until("delete", "the toast", () => [...document.querySelectorAll('[role="status"]')].some((e) => /^Deleted/.test(e.textContent.trim())));
  await w.check("a toast says it was deleted");
  // F6 visits the toast while it offers Undo (§5.6); Tab alone would be 20 stops away.
  for (let i = 0; i < 5 && !(await page.evaluate(() => document.activeElement?.textContent?.trim() === "Undo")); i++) await w.key("F6");
  await w.expect("undo", "F6 to reach the Undo button of the toast", () => page.evaluate(() => document.activeElement?.textContent?.trim() === "Undo"));
  await w.check("Undo has the focus");
  await w.key("Enter");
  await w.until("undo", "the snippet back", () => [...document.querySelectorAll('[role="option"]')].length >= 4);
  await w.until("undo", "the focus to be on something when the toast has gone", () => document.activeElement !== document.body && !document.querySelector(".toast"));
  await w.check("after Undo");

  // 8. Settings (Ctrl+,): the focus goes in; change the appearance with the arrow keys.
  await w.key("Control+,");
  await w.until("settings", "the Settings heading", () => document.querySelector("h1")?.textContent === "Settings");
  await w.until("settings", "the focus on the settings title when it opens from a shortcut", () => document.activeElement === document.querySelector("h1"));
  await w.check("settings opened");
  for (const section of ["Typing", "Dates", "Shortcuts", "Apps", "Privacy", "About", "General"]) {
    await w.tabTo("settings", `the ${section} button`, (name) => document.activeElement?.closest(".settings-nav") && document.activeElement.textContent.trim() === name, section, { back: section === "General" });
    await w.key("Enter");
    await w.until("settings", `the ${section} section`, (name) => document.querySelector("#settings-section")?.textContent.trim() === name, section);
    await w.check(`settings: ${section}`);
  }
  await w.tabTo("settings", "the Appearance choice", () => document.activeElement?.type === "radio" && document.activeElement.closest('[role="radiogroup"]'));
  await w.check("a setting has the focus");
  await w.key("ArrowRight");
  await w.until("settings", "the appearance stored", () => window.__mock.state.puts.some(([k]) => k === "ui.theme"));
  await w.until("settings", "the theme applied", () => document.documentElement.dataset.theme === "light" || document.documentElement.dataset.theme === "dark");
  await w.check("the appearance changed");
  await w.key("ArrowLeft");

  // 9. Library health and Why didn't it expand? (from the sidebar, by Tab and Enter).
  await w.key("Control+1");
  for (const name of ["Library health", "Why didn’t it expand?"]) {
    await w.tabTo("views", `the ${name} button`, (n) => document.activeElement?.closest("[data-pane='sidebar']") && document.activeElement.textContent.includes(n), name);
    await w.key("Enter");
    await w.until("views", name, (n) => [...document.querySelectorAll("h1")].some((h) => h.textContent.includes(n)), name);
    await w.check(name);
  }

  // 10. The window at 200% zoom (1280 x 800 at 200% is 640 x 400 CSS pixels): nothing scrolls sideways (1.4.4, 1.4.10).
  await page.setViewportSize({ width: 640, height: 400 });
  await w.key("Control+1");
  await w.tabTo("zoom 200%", "Library health", () => document.activeElement?.textContent.includes("Library health"), undefined, { back: true, max: 6 });
  const sideways = await page.evaluate(() => {
    const bad = [];
    for (const el of document.querySelectorAll("*")) {
      const s = getComputedStyle(el);
      if ((s.overflowX === "auto" || s.overflowX === "scroll") && el.scrollWidth > el.clientWidth + 1) bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} ${el.scrollWidth}>${el.clientWidth}`);
    }
    if (document.documentElement.scrollWidth > innerWidth + 1) bad.push("the page");
    return bad;
  });
  if (sideways.length) w.fail("zoom 200%", `scrolls sideways: ${sideways.join(", ")}`);
  await w.check("zoom 200%");
  await page.setViewportSize({ width: 1280, height: 800 });

  if (errors.length) w.fail("page errors", errors.join(" | "));
  await context.close();
}

async function walkPalette(browser, mode, snippet) {
  const open_ = { session: 1, ownWindow: false, app: "Mail", lastForm: null };
  const { context, page, errors } = await open(browser, mode, { window: "palette", snippets: SEED.concat([snippet]), open: open_ });
  const w = new Walk(page, mode);
  await page.waitForSelector('[role="dialog"]');
  await page.evaluate(() => window.__emit("iamb:palette", { type: "shown", session: 1 }));
  await w.until("palette", "the search box to have the focus", () => document.activeElement?.getAttribute("role") === "combobox");
  await w.check("the palette opens in the search box");
  await w.type("greet");
  await w.until("palette", "a result", () => document.querySelectorAll('[role="option"]').length > 0);
  await w.until("palette", "the count in a live region", () => [...document.querySelectorAll('[role="status"],[aria-live]')].some((e) => /\d+ (result|of)/.test(e.textContent)));
  await w.check("a result is found");
  await w.key("ArrowDown");
  await w.key("ArrowUp");
  await w.check("the arrow keys move the choice");
  await w.key("Enter");
  await w.until("palette", "the pick", () => window.__mock.state.picked.length > 0);
  const pick = await page.evaluate(() => window.__mock.state.picked.at(-1));
  if (pick?.kind !== "insert" && pick?.kind !== "fill_in" && pick?.kind !== "form") w.fail("palette", `Enter did not pick the snippet: ${JSON.stringify(pick)}`);
  if (errors.length) w.fail("page errors", errors.join(" | "));
  await context.close();
}

async function walkForm(browser, mode) {
  const form = {
    session: 3, snippet: "00000000-0000-7000-8000-000000000001", label: "Repair quote", abbreviation: ";quote", app: "Mail",
    fields: [
      { name: "Customer", kind: "text", options: [], default: "" },
      { name: "Bike", kind: "choice", options: ["Road", "Gravel", "City"], default: "Gravel" },
      { name: "Work needed", kind: "area", options: [], default: "" },
      { name: "Parts in stock", kind: "toggle", options: [], default: "no" },
    ],
    lastValues: null,
  };
  const { context, page, errors } = await open(browser, mode, { window: "form", snippets: SEED, open: form });
  const w = new Walk(page, mode);
  await page.waitForSelector('[role="dialog"]');
  await page.evaluate(() => window.__emit("iamb:form", { type: "shown", session: 3 }));
  await w.until("form", "the first field to have the focus", () => document.activeElement?.getAttribute("data-field") === "0");
  await w.check("the form opens in the first field");
  await w.type("Ana Silva");
  await w.key("Tab");
  await w.check("the choice");
  await w.key("ArrowDown");
  await w.key("Tab");
  await w.type("Fix the rear brake");
  await w.check("the several-line field");
  await w.key("Tab");
  await w.key("Space");
  await w.check("the toggle");
  await w.key("Shift+Tab");
  await w.expect("form", "Shift+Tab to go back a field", () => page.evaluate(() => document.activeElement?.tagName === "TEXTAREA"));
  await w.key("Control+Enter");
  await w.until("form", "the form submitted", () => window.__mock.state.submitted.length > 0);
  const v = await page.evaluate(() => window.__mock.state.submitted.at(-1));
  if (v?.Customer !== "Ana Silva" || v?.["Parts in stock"] !== "yes" || !/brake/.test(v?.["Work needed"] ?? "")) w.fail("form", `the answers were not what was typed: ${JSON.stringify(v)}`);
  if (errors.length) w.fail("page errors", errors.join(" | "));
  await context.close();

  // Esc cancels (a second panel, as the first is gone once it has inserted).
  const again = await open(browser, mode, { window: "form", snippets: SEED, open: form });
  const w2 = new Walk(again.page, mode);
  await again.page.waitForSelector('[role="dialog"]');
  await again.page.evaluate(() => window.__emit("iamb:form", { type: "shown", session: 3 }));
  await w2.until("form", "the first field to have the focus", () => document.activeElement?.getAttribute("data-field") === "0");
  await w2.key("Escape");
  await w2.until("form", "the form cancelled", () => window.__mock.state.cancelled > 0);
  await again.context.close();
}

async function walkOnboarding(browser, mode) {
  const { context, page, errors } = await open(browser, mode, { window: "onboarding", snippets: [] });
  const w = new Walk(page, mode);
  await page.waitForSelector("h1");
  await w.until("onboarding", "Get started to have the focus", () => /Get started/.test(document.activeElement?.textContent ?? ""));
  await w.check("onboarding: welcome");
  await w.key("Enter");
  await w.until("onboarding", "the snippet step", () => /Make your first snippet/.test(document.querySelector("h1")?.textContent ?? ""));
  await w.check("onboarding: first snippet");
  if (errors.length) w.fail("page errors", errors.join(" | "));
  await context.close();
}

const browser = await launch();
for (const mode of MODES) {
  if (only && mode.name !== only) continue;
  if (!asJson) console.log(`${mode.name}`);
  await walkLibrary(browser, mode);
  await walkPalette(browser, mode, { abbreviation: ";greet", label: "Greeting for a customer", body: "Hello" });
  await walkForm(browser, mode);
  await walkOnboarding(browser, mode);
}
await browser.close();
server.close();

const total = (impact) => results.reduce((n, r) => n + r.counts[impact], 0);
const left = new Map();
for (const r of results) for (const f of r.left) left.set(`${f.id}`, `${f.impact}: ${f.help} (${r.mode}, ${r.step}: ${f.nodes.join(" | ")})`);
if (asJson) {
  console.log(JSON.stringify({ failures, results }, null, 1));
} else {
  console.log(`\naxe over ${results.length} steps: critical ${total("critical")}, serious ${total("serious")}, moderate ${total("moderate")}, minor ${total("minor")}`);
  for (const [id, text] of left) console.log(`  left: ${id} ${text}`);
  console.log(failures.length ? `\n${failures.length} FAILED` : "\nkeyboard-only run passed");
}
process.exit(failures.length ? 1 : 0);
