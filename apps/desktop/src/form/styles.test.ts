import { describe, expect, it } from "vitest";

import { stylesheet, undefinedTokens } from "../../scripts/stylesheet.mjs";

describe("the form panel's stylesheet", () => {
  it("uses only design tokens that exist (tokens.css, from docs/ux/tokens.json)", () => {
    const { used, missing } = undefinedTokens("form/form.css");
    expect(used).toBeGreaterThan(20);
    expect(missing).toEqual([]);
  });

  it("never animates the panel (duration.instant: no transition, no animation)", () => {
    expect(stylesheet("form/form.css")).not.toMatch(/transition|animation/);
  });
});
