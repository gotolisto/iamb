import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("picks the view by the window's label", () => {
    expect(renderToString(<App label="library" />)).toContain('aria-label="Library"');
    expect(renderToString(<App label="form" />)).toContain('class="ff-root"');
    expect(renderToString(<App label="palette" />)).toContain("Search snippets");
  });

  it("renders an empty form panel before a request", () => {
    expect(renderToString(<App label="form" />)).toBe('<div class="ff-root"></div>');
  });
});
