// The automatic part of the paste-fidelity matrix (docs/paste-fidelity.md): every fixture's
// clipboard content, exactly as Iamb builds it (`clipboard.json`, written by
// `crates/iamb-service/tests/paste_fidelity.rs`), is put on a headless browser's clipboard as
// HTML and plain text, pasted with the paste shortcut into a rich-text editor (a contenteditable
// element, the kind of field web mail and document apps use) and into a plain text field, and
// the result is checked: formatting, links, lists, the inline image, and the plain-text fallback.
//
//   npm install --no-save playwright@1.63.0   # once; uses the installed Google Chrome
//   node docs/paste-fidelity/browser-paste.mjs
//
// IAMB_CHROME_CHANNEL picks another browser channel (default `chrome`); IAMB_CHROME_PATH an
// executable. Exits 1 when any check fails.

import { readFileSync } from "node:fs";

import { chromium } from "playwright";

const fixtures = JSON.parse(readFileSync(new URL("./clipboard.json", import.meta.url), "utf8"));

/** What the rich editor must hold after the paste, per fixture (run in the page). */
const CHECKS = {
  ";fx-head": `
    q("h1")?.textContent === "Order update" && text().includes("Your order has shipped.")`,
  ";fx-bold": `bold("Kim") && text() === "Thanks for your order, Kim."`,
  ";fx-italic": `italic("really") && text() === "We are really sorry about the delay."`,
  ";fx-link": `
    link("https://example.com/track", "our site") &&
    link("mailto:orders@example.com", "orders@example.com")`,
  ";fx-list": `count("ul > li") === 3 && q("ul > li")?.textContent === "the box"`,
  ";fx-nested": `
    count("ol > li") === 3 && count("ol > li ul > li") === 2 &&
    q("ol > li ul > li")?.textContent === "the box"`,
  ";fx-image": `
    image() && lines().includes("Kim Lee") && lines().includes("Orders team")`,
  ";fx-field": `
    bold("Ada <Lovelace> & *co*") && italic("blue mug") && count("a, img, h1, ul, ol") === 0`,
  ";fx-sig": `
    bold("Kim Lee") && italic("Orders team") && link("https://example.com", "example.com") &&
    struck("old number") && image()`,
};

/** Helpers the checks use, defined in the page. */
const HELPERS = `
  const root = document.getElementById("rich");
  const q = (s) => root.querySelector(s);
  const count = (s) => root.querySelectorAll(s).length;
  const text = () => root.innerText.replace(/\\s+/g, " ").trim();
  const lines = () => root.innerText.split("\\n").map((l) => l.trim());
  const styled = (pred, wanted) =>
    [...root.querySelectorAll("*")].some((el) => el.textContent === wanted && pred(getComputedStyle(el)));
  const bold = (w) => styled((s) => Number(s.fontWeight) >= 600, w);
  const italic = (w) => styled((s) => s.fontStyle === "italic", w);
  const struck = (w) => styled((s) => s.textDecorationLine.includes("line-through"), w);
  const link = (href, label) =>
    [...root.querySelectorAll("a")].some((a) => a.href === new URL(href).href && a.textContent === label);
  const image = () =>
    [...root.querySelectorAll("img")].some(
      (img) => img.src.startsWith("data:image/png;base64,") && img.complete && img.naturalWidth === 32,
    );
`;

const PAGE = `<!doctype html><meta charset="utf-8"><title>Paste fidelity</title>
<div id="rich" contenteditable="true" style="min-height:4em;border:1px solid"></div>
<textarea id="plain" rows="6" cols="60"></textarea>`;

const browser = await chromium.launch({
  channel: process.env.IAMB_CHROME_PATH ? undefined : (process.env.IAMB_CHROME_CHANNEL ?? "chrome"),
  executablePath: process.env.IAMB_CHROME_PATH,
});
const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
const page = await context.newPage();
// A secure origin, so the page may write the clipboard.
await page.route("https://paste-fidelity.test/", (route) =>
  route.fulfill({ status: 200, contentType: "text/html", body: PAGE }),
);
await page.goto("https://paste-fidelity.test/");

const paste = process.platform === "darwin" ? "Meta+V" : "Control+V";
const results = [];
for (const f of fixtures) {
  await page.evaluate(async ({ html, plain }) => {
    await navigator.clipboard.write([
      new ClipboardItem({
        "text/html": new Blob([html], { type: "text/html" }),
        "text/plain": new Blob([plain], { type: "text/plain" }),
      }),
    ]);
  }, f);

  await page.evaluate(() => {
    const el = document.getElementById("rich");
    el.innerHTML = "";
    el.focus();
  });
  await page.keyboard.press(paste);
  await page.waitForTimeout(100);
  const check = CHECKS[f.abbreviation];
  const rich = check
    ? await page.evaluate(`(async () => { ${HELPERS}
        await Promise.all([...root.querySelectorAll("img")].map((i) => i.decode().catch(() => {})));
        return Boolean(${check}); })()`)
    : false;

  await page.fill("#plain", "");
  await page.focus("#plain");
  await page.keyboard.press(paste);
  const typed = await page.inputValue("#plain");
  const plain = typed.replace(/\r\n?/g, "\n") === f.plain;

  results.push({ fixture: f.abbreviation, rich, plain });
}
await browser.close();

let failed = 0;
for (const r of results) {
  const ok = r.rich && r.plain;
  if (!ok) failed += 1;
  console.log(
    `${ok ? "pass" : "FAIL"}  ${r.fixture.padEnd(12)} formatted ${r.rich ? "ok" : "wrong"} · plain ${r.plain ? "ok" : "wrong"}`,
  );
}
console.log(`${results.length - failed} of ${results.length} fixtures pass in the headless browser`);
process.exit(failed === 0 && results.length === Object.keys(CHECKS).length ? 0 : 1);
