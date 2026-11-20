import { describe, expect, it } from "vitest";

import { stylesheet, undefinedTokens } from "../../scripts/stylesheet.mjs";

describe("the onboarding window's stylesheet", () => {
  it("uses only design tokens that exist (tokens.css, from docs/ux/tokens.json)", () => {
    const { used, missing } = undefinedTokens("onboarding/onboarding.css");
    expect(used).toBeGreaterThan(20);
    expect(missing).toEqual([]);
  });

  it("never animates anything (duration.instant: no transition, no animation)", () => {
    expect(stylesheet("onboarding/onboarding.css")).not.toMatch(/transition|animation/);
  });
});
