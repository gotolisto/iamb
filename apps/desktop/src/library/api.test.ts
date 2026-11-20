import { describe, expect, it } from "vitest";

import { COMMANDS, isUiEvent } from "../ipc";
import {
  createLibraryApi,
  isAbbreviationCheck,
  isFolders,
  isHealth,
  isSaved,
  isSnippet,
  isSummary,
  isSummaryPage,
  isTag,
} from "./api";
import fixtures from "./library.fixtures.json";

describe("the payloads of the library commands (written by tests/library_contract.rs)", () => {
  it("pass the front end's guards", () => {
    expect(isFolders(fixtures.folders)).toBe(true);
    expect(fixtures.tags.every(isTag)).toBe(true);
    expect(isSummaryPage(fixtures.page)).toBe(true);
    expect(fixtures.page.summaries.every(isSummary)).toBe(true);
    expect(isSnippet(fixtures.snippet)).toBe(true);
    expect(isSaved(fixtures.saved)).toBe(true);
    expect(isAbbreviationCheck(fixtures.check)).toBe(true);
    expect(isHealth(fixtures.health)).toBe(true);
  });

  it("are refused when a field is wrong", () => {
    expect(isFolders({ ...fixtures.folders, profile: 1 })).toBe(false);
    expect(isTag({ ...fixtures.tags[0], color: "pink" })).toBe(false);
    expect(isSummary({ ...fixtures.page.summaries[0], broken: 3 })).toBe(false);
    expect(isSnippet({ ...fixtures.snippet, trigger: "sometimes" })).toBe(false);
    expect(isSaved({ id: "x" })).toBe(false);
    expect(isAbbreviationCheck({ conflict: null, hides: "x", hiddenBy: [] })).toBe(false);
    expect(isHealth({ ...fixtures.health, unused: [{ snippet: 1, lastUsed: null }] })).toBe(false);
    expect(isHealth({ ...fixtures.health, excluded: [{ snippet: "a" }] })).toBe(false);
  });
});

describe("the library events", () => {
  it("carry the rows a change touched, or nothing when anything may have changed", () => {
    expect(isUiEvent({ type: "library_changed", revision: 2, changed: null })).toBe(true);
    expect(
      isUiEvent({ type: "library_changed", revision: 2, changed: { snippets: ["a"], folders: [], tags: [] } }),
    ).toBe(true);
    expect(isUiEvent({ type: "library_changed", revision: 2, changed: { snippets: "a" } })).toBe(false);
    expect(isUiEvent({ type: "library_changed", revision: 2 })).toBe(false);
  });
});

describe("the commands", () => {
  it("call the Rust commands by name with the arguments they take", async () => {
    const calls: [string, unknown][] = [];
    const api = createLibraryApi(async (command, args) => {
      calls.push([command, args]);
      return undefined as never;
    });
    await api.listFolders();
    await api.listTags();
    await api.listSummaries(1000, 1000);
    await api.getSnippet("i");
    await api.deleteSnippet("i");
    await api.setEnabled("i", false);
    await api.deleteFolder("f");
    await api.deleteTag("t");
    await api.checkAbbreviation(";r", "immediate");
    await api.checkAbbreviation(";r", "delimiter", "i");
    await api.saveTag({ name: "n", color: "red" });
    await api.health();
    await api.resetUsage();
    expect(calls).toEqual([
      ["library_list_folders", undefined],
      ["library_list_tags", undefined],
      ["library_list_summaries", { offset: 1000, limit: 1000 }],
      ["library_get_snippet", { id: "i" }],
      ["library_delete_snippet", { id: "i" }],
      ["library_set_enabled", { id: "i", enabled: false }],
      ["library_delete_folder", { id: "f" }],
      ["library_delete_tag", { id: "t" }],
      ["library_check_abbreviation", { abbreviation: ";r", trigger: "immediate", snippet: null }],
      ["library_check_abbreviation", { abbreviation: ";r", trigger: "delimiter", snippet: "i" }],
      ["library_save_tag", { draft: { name: "n", color: "red" } }],
      ["library_health", undefined],
      ["library_reset_usage", undefined],
    ]);
    // Every command the front end calls is one Rust registers (tests/front_end_contract.rs).
    for (const [command] of calls) expect(COMMANDS as readonly string[]).toContain(command);
  });
});
