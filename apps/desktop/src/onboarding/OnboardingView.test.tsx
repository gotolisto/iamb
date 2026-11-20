// @vitest-environment jsdom
import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createFake, renderLibrary } from "../library/testkit";
import { LIBRARY_KEY, LIBRARY_OPEN_KEY } from "./model";
import { modeOf, rememberSelection, requestDiagnostics } from "./OnboardingView";

afterEach(() => window.localStorage.clear());

describe("the window's mode", () => {
  it("is the permissions step alone for ?mode=fix, every step otherwise", () => {
    expect(modeOf("?mode=fix")).toBe("fix");
    expect(modeOf("")).toBe("setup");
    expect(modeOf("?mode=other")).toBe("setup");
  });
});

describe("what the library window is told", () => {
  it("opens with the new snippet selected, keeping its sort order", async () => {
    const fake = createFake({ snippets: [{ abbreviation: ";em", label: "Email", body: "a" }, { abbreviation: ";x", label: "Zed", body: "b" }] });
    const id = fake.byAbbreviation(";em")?.id as string;
    window.localStorage.setItem(LIBRARY_KEY, JSON.stringify({ sort: "changed", view: null, snippet: null }));
    rememberSelection(window.localStorage, id);
    expect(JSON.parse(window.localStorage.getItem(LIBRARY_KEY) ?? "{}")).toEqual({ sort: "changed", view: null, snippet: id });
    await renderLibrary(fake);
    const selected = await screen.findByRole("option", { name: /^Email/ });
    expect(selected.getAttribute("aria-selected")).toBe("true");
  });

  it("opens at “Why didn’t it expand?” once, when the request is fresh", async () => {
    requestDiagnostics(window.localStorage, Date.now());
    const first = await renderLibrary(createFake());
    expect(await screen.findByRole("heading", { level: 1, name: "Why didn’t it expand?" })).toBeTruthy();
    expect(window.localStorage.getItem(LIBRARY_OPEN_KEY)).toBeNull();
    first.unmount();
    const second = await renderLibrary(createFake());
    expect(screen.queryByRole("heading", { level: 1, name: "Why didn’t it expand?" })).toBeNull();
    second.unmount();
  });

  it("ignores a request that is old", async () => {
    requestDiagnostics(window.localStorage, Date.now() - 60_000);
    await renderLibrary(createFake());
    expect(screen.queryByRole("heading", { level: 1, name: "Why didn’t it expand?" })).toBeNull();
  });
});
