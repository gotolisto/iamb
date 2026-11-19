// Generates src/styles/tokens.css from the design tokens in docs/ux/tokens.json, the same way
// docs/ux/mockups/build.py generates the mock-ups' custom properties (docs/ux/design-system.md
// §2: a token's path joined with hyphens, without the theme; light values on :root, dark values
// under prefers-color-scheme unless the user chose light, and under [data-theme="dark"]).
//
//   node scripts/tokens.mjs           write src/styles/tokens.css
//   node scripts/tokens.mjs --check   exit 1 when the file is out of date
//
// `src/styles/tokens.test.ts` runs the check, and checks that base.css uses only defined tokens.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const TOKENS_JSON = join(here, "../../../docs/ux/tokens.json");
export const TOKENS_CSS = join(here, "../src/styles/tokens.css");

const cssColor = (v) => {
  if (v.alpha === undefined || v.alpha === null) return v.hex;
  const [r, g, b] = v.components.map((c) => Math.round(c * 255));
  return `rgb(${r} ${g} ${b} / ${v.alpha})`;
};

const cssDim = (v) => `${v.value}${v.unit}`;

const cssShadow = (v) =>
  (Array.isArray(v) ? v : [v])
    .map((s) =>
      [cssDim(s.offsetX), cssDim(s.offsetY), cssDim(s.blur), cssDim(s.spread), cssColor(s.color)].join(
        " ",
      ),
    )
    .join(", ");

const resolve = (tokens, ref) => {
  let node = tokens;
  for (const part of ref.replace(/^\{|\}$/g, "").split(".")) node = node[part];
  return node.$value;
};

function cssValue(tokens, kind, value) {
  switch (kind) {
    case "color":
      return cssColor(value);
    case "dimension":
    case "duration":
      return cssDim(value);
    case "shadow":
      return cssShadow(value);
    case "fontFamily":
      return value.map((f) => (f.includes(" ") ? `"${f}"` : f)).join(", ");
    case "cubicBezier":
      return `cubic-bezier(${value.join(", ")})`;
    case "typography": {
      const family = value.fontFamily.replace(/^\{|\}$/g, "").replaceAll(".", "-");
      return (
        `${resolve(tokens, value.fontWeight)} ${cssDim(resolve(tokens, value.fontSize))}` +
        `/${resolve(tokens, value.lineHeight)} var(--${family})`
      );
    }
    default:
      return String(value);
  }
}

function walk(node, path, kind, out) {
  const k = node.$type ?? kind;
  if ("$value" in node) {
    out.push([path, k ?? "", node.$value]);
    return;
  }
  for (const [key, child] of Object.entries(node)) {
    if (!key.startsWith("$") && child && typeof child === "object" && !Array.isArray(child)) {
      walk(child, [...path, key], k, out);
    }
  }
}

/** The stylesheet for `tokens` (the parsed tokens.json). */
export function tokenCss(tokens) {
  const flat = [];
  walk(tokens, [], undefined, flat);
  const shared = [];
  const themed = { light: [], dark: [] };
  for (const [path, kind, value] of flat) {
    if ((path[0] === "color" || path[0] === "shadow") && path[1] in themed) {
      const name = [path[0], ...path.slice(2)].join("-");
      themed[path[1]].push(`--${name}: ${cssValue(tokens, kind, value)};`);
      continue;
    }
    if (kind === "typography") {
      const name = `type-${path.slice(1).join("-")}`;
      shared.push(`--${name}: ${cssValue(tokens, kind, value)};`);
      if (value.letterSpacing?.value) shared.push(`--${name}-tracking: ${cssDim(value.letterSpacing)};`);
      continue;
    }
    shared.push(`--${path.join("-")}: ${cssValue(tokens, kind, value)};`);
  }
  const block = (selector, lines, indent = "") =>
    `${indent}${selector} {\n${lines.map((l) => `${indent}  ${l}`).join("\n")}\n${indent}}`;
  return (
    [
      "/* Generated from docs/ux/tokens.json by apps/desktop/scripts/tokens.mjs. Do not edit here. */",
      block(":root", [...shared, ...themed.light, "color-scheme: light;"]),
      "@media (prefers-color-scheme: dark) {",
      block(':root:not([data-theme="light"])', [...themed.dark, "color-scheme: dark;"], "  "),
      "}",
      block(':root[data-theme="dark"]', [...themed.dark, "color-scheme: dark;"]),
    ].join("\n") + "\n"
  );
}

/** The stylesheet the current tokens.json gives. */
export function generated() {
  return tokenCss(JSON.parse(readFileSync(TOKENS_JSON, "utf8")));
}

/** The committed stylesheet. */
export function committed() {
  return readFileSync(TOKENS_CSS, "utf8");
}

/** The custom properties `src/styles/base.css` uses that the stylesheet does not define. */
export function missingInBase() {
  const tokens = new Set([...committed().matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]));
  const base = readFileSync(join(here, "../src/styles/base.css"), "utf8");
  const used = new Set([...base.matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]));
  return [...used].filter((t) => !tokens.has(t));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const css = generated();
  if (process.argv.includes("--check")) {
    if (committed() !== css) {
      console.error("src/styles/tokens.css is out of date: run node scripts/tokens.mjs");
      process.exit(1);
    }
  } else {
    writeFileSync(TOKENS_CSS, css);
  }
}
