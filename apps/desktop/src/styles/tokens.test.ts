import { describe, expect, it } from "vitest";

import { committed, generated, missingInBase } from "../../scripts/tokens.mjs";

describe("design tokens", () => {
  it("tokens.css is generated from docs/ux/tokens.json (run node scripts/tokens.mjs)", () => {
    expect(committed()).toBe(generated());
  });

  it("base.css uses only tokens that exist", () => {
    expect(missingInBase()).toEqual([]);
  });
});
