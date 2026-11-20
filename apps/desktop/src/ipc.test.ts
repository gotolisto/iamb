import { describe, expect, it } from "vitest";

import fixtures from "./contract.fixtures.json";
import {
  COMMANDS,
  createApi,
  isCommandError,
  isStatusView,
  isUiEvent,
  STATUS_EVENT,
  UI_EVENT,
  type CommandName,
} from "./ipc";

function recordingApi() {
  const calls: Array<{ command: CommandName; args: Record<string, unknown> | undefined }> = [];
  const api = createApi(<T>(command: CommandName, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    return Promise.resolve(undefined as T);
  });
  return { api, calls };
}

describe("commands", () => {
  it("call the Rust commands by name with camel-case arguments", async () => {
    const { api, calls } = recordingApi();
    await api.status();
    await api.refreshStatus();
    await api.setPaused(true);
    await api.listSnippets();
    await api.listSnippets("02020202-0202-0202-0202-020202020202");
    await api.getSnippet("01010101-0101-0101-0101-010101010101");
    await api.reportFirstPaint({ navigationAtMs: 1000, scriptAtMs: 1100, paintedAtMs: 1234.5 });
    await api.dismiss();
    expect(calls).toEqual([
      { command: "status", args: undefined },
      { command: "refresh_status", args: undefined },
      { command: "set_paused", args: { paused: true } },
      { command: "list_snippets", args: { folder: null } },
      { command: "list_snippets", args: { folder: "02020202-0202-0202-0202-020202020202" } },
      { command: "get_snippet", args: { id: "01010101-0101-0101-0101-010101010101" } },
      {
        command: "report_first_paint",
        args: { navigationAtMs: 1000, scriptAtMs: 1100, paintedAtMs: 1234.5 },
      },
      { command: "dismiss", args: undefined },
    ]);
    // Every registered command is reachable from the API (the library's from its own,
    // library/api.test.ts; the template editor's from library/template/api.test.ts; the form
    // panel's from form/api.test.ts; Settings and the log's from library/settings/api.test.ts; the onboarding window's from
    // onboarding/api.test.ts; the palette's from palette/api.test.ts).
    expect(new Set(calls.map((c) => c.command))).toEqual(
      new Set(
        COMMANDS.filter(
          (c) =>
            !c.startsWith("template_") &&
            !c.startsWith("library_") &&
            !c.startsWith("form_") &&
            !c.startsWith("settings_") &&
            !c.startsWith("diagnostics_") &&
            !c.startsWith("onboarding_") &&
            !c.startsWith("palette_"),
        ),
      ),
    );
  });

  it("name the two events the Rust side emits", () => {
    expect(STATUS_EVENT).toBe("iamb:status");
    expect(UI_EVENT).toBe("iamb:event");
  });
});

describe("payloads written by the Rust side (contract.fixtures.json)", () => {
  it("are accepted by the guards", () => {
    expect(fixtures.statuses).toHaveLength(3);
    for (const status of fixtures.statuses) expect(isStatusView(status)).toBe(true);
    expect(fixtures.events).toHaveLength(5);
    for (const event of fixtures.events) expect(isUiEvent(event)).toBe(true);
    expect(isCommandError(fixtures.error)).toBe(true);
    const snippet = fixtures.snippet;
    expect(snippet.abbreviation).toBe("brb");
    expect(snippet.folderId).toMatch(/^[0-9a-f-]{36}$/);
    expect(fixtures.snippets[0]?.label).toBe("Back soon");
  });

  it("are rejected when their shape changes", () => {
    const connected = fixtures.statuses[2];
    expect(isStatusView({ ...connected, connection: "online" })).toBe(false);
    expect(isStatusView({ ...connected, service: { version: "1" } })).toBe(false);
    expect(isUiEvent({ type: "library_changed", revision: "4" })).toBe(false);
    expect(isUiEvent({ type: "form_requested", session: 1, snippet: "x", fields: [{}] })).toBe(
      false,
    );
    expect(isUiEvent({ type: "outbox_changed", pending: 1 })).toBe(false);
    expect(isUiEvent(null)).toBe(false);
    expect(isCommandError({ code: 1, message: "x" })).toBe(false);
  });
});
