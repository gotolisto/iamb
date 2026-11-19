import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("picks the view by the window's label", () => {
    expect(renderToString(<App label="library" />)).toContain("Service: connecting");
    expect(renderToString(<App label="form" />)).toContain("Fill-in form");
    expect(renderToString(<App label="palette" />)).toContain("Search snippets");
  });

  it("never shows a value in the form placeholder before a request", () => {
    expect(renderToString(<App label="form" />)).toContain("No form is open.");
  });
});
