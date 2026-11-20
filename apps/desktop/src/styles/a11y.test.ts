import { describe, expect, it } from "vitest";

import { stylesheet, stylesheets } from "../../scripts/stylesheet.mjs";

// What the style sheets owe the accessibility commitments (docs/ux/spec.md §8, design system
// §2.6 and §5): colours come from the tokens, so that the light and dark themes and the contrast
// checks of §2.2 apply to everything; motion stops when the system asks; contrast themes get
// their own boundaries.

const WINDOW_SHEETS = ["library/library.css", "palette/palette.css", "form/form.css", "onboarding/onboarding.css"];

const withoutComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");

/** The values of every declaration, with the names of custom properties taken out. */
function values(css: string): string[] {
  const out: string[] = [];
  for (const m of withoutComments(css).matchAll(/([a-z-]+)\s*:\s*([^;{}]+)[;}]/g)) {
    out.push(`${m[1]}: ${(m[2] ?? "").replace(/var\(--[a-z0-9-]+/g, "var(")}`);
  }
  return out;
}

const COLOUR_FUNCTION = /(#[0-9a-fA-F]{3,8}\b|\b(rgb|rgba|hsl|hsla|hwb|lab|lch|oklab|oklch|color|color-mix)\()/;
const NAMED = /\b(white|black|red|green|blue|yellow|orange|purple|pink|brown|silver|gold|navy|teal|maroon|olive|lime|aqua|cyan|magenta|fuchsia|gr[ae]y|crimson|indigo|violet)\b/i;

describe("style sheets", () => {
  it("would see a raw colour (so that a pass means something)", () => {
    const found = values(".a { color: #fff; border: 1px solid red; background: var(--color-bg-app); outline: 2px solid Highlight }");
    expect(found.filter((v) => COLOUR_FUNCTION.test(v))).toEqual(["color: #fff"]);
    expect(found.filter((v) => NAMED.test(v))).toEqual(["border: 1px solid red"]);
  });

  const sheets = stylesheets().filter((p) => p !== "styles/tokens.css");

  it("finds the style sheets of every window", () => {
    for (const path of WINDOW_SHEETS) expect(sheets).toContain(path);
    expect(sheets.length).toBeGreaterThanOrEqual(9);
  });

  it("uses no colour that is not a token (only tokens.css has colour values)", () => {
    const raw: string[] = [];
    for (const path of sheets) {
      for (const v of values(stylesheet(path))) {
        // A `content` string or a url is not a colour.
        if (/^content:|url\(/.test(v)) continue;
        if (COLOUR_FUNCTION.test(v) || (/^(color|background|background-color|border|border-[a-z-]+|outline|outline-color|fill|stroke|box-shadow|text-shadow|caret-color|accent-color|text-decoration-color):/.test(v) && NAMED.test(v))) {
          raw.push(`${path}: ${v.trim()}`);
        }
      }
    }
    expect(raw).toEqual([]);
  });

  it("has the colours of both themes: dark by the system and dark chosen in Settings carry the same values", () => {
    const tokens = withoutComments(stylesheet("styles/tokens.css"));
    const block = (open: string) => {
      const at = tokens.indexOf(open);
      expect(at).toBeGreaterThan(-1);
      const end = tokens.indexOf("}", at);
      return tokens
        .slice(tokens.indexOf("{", at) + 1, end)
        .split(";")
        .map((d) => d.trim())
        .filter(Boolean);
    };
    const bySystem = block(':root:not([data-theme="light"])');
    const chosen = block(':root[data-theme="dark"]');
    // The chosen theme restates every colour; the system one only the ones that differ from light.
    for (const d of bySystem) expect(chosen).toContain(d);
    expect(tokens).toMatch(/@media \(prefers-color-scheme: dark\)/);
    expect(chosen.length).toBeGreaterThanOrEqual(bySystem.length);
  });

  it("turns every duration to 0 when the system asks for less motion", () => {
    const base = withoutComments(stylesheet("styles/base.css"));
    const reduced = base.slice(base.indexOf("@media (prefers-reduced-motion: reduce)"));
    expect(reduced).toMatch(/transition-duration:\s*0s !important/);
    expect(reduced).toMatch(/animation-duration:\s*0s !important/);
    expect(reduced).toMatch(/scroll-behavior:\s*auto !important/);
  });

  it("animates nothing but with the duration tokens, and only transforms and opacity (design system §2.6)", () => {
    for (const path of sheets) {
      const css = withoutComments(stylesheet(path));
      expect(css, `${path} has keyframes: add them to the reduced-motion test`).not.toMatch(/@keyframes/);
      for (const m of css.matchAll(/transition\s*:\s*([^;]+);/g)) {
        expect(m[1], `${path}: ${m[0]}`).toMatch(/^(none|(transform|opacity)\b.*var\(--duration-)/);
      }
    }
  });

  it("gives every window a contrast-theme block (forced-colors) that keeps the selection and the focus ring", () => {
    for (const path of WINDOW_SHEETS) {
      const css = withoutComments(stylesheet(path));
      expect(css, path).toMatch(/@media \(forced-colors: active\)/);
      expect(css, path).toMatch(/CanvasText|Highlight/);
    }
    expect(withoutComments(stylesheet("styles/base.css"))).toMatch(/@media \(forced-colors: active\)[\s\S]*outline-color: Highlight/);
  });

  it("never removes the focus ring without drawing another one", () => {
    for (const path of ["styles/base.css", ...sheets]) {
      const css = withoutComments(stylesheet(path));
      // Rules that switch an outline off on focus: each must be a field whose ring is drawn
      // around its wrapper or by its border.
      const removed = [...css.matchAll(/([^{}]+):focus(?:-visible)?\s*\{[^}]*outline:\s*(none|0)\b[^}]*\}/g)].map((m) => (m[1] ?? "").trim());
      for (const selector of removed) {
        const stem = selector.replace(/:focus.*$/, "");
        expect(css, `${path}: ${selector} has no outline and nothing else shows the focus`).toMatch(
          new RegExp(`${stem.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[^{}]*:focus-within|:focus-within`),
        );
      }
    }
  });
});
