import { describe, expect, it } from "vitest";

import fixtures from "../contract.fixtures.json";
import { COMMANDS, type CommandName } from "../ipc";
import { createFormApi, FOCUS_LOST, FORM_EVENT, isFocusLost, isFormEvent, isFormOpen } from "./api";

describe("the form panel's commands", () => {
  it("call the Rust commands by name with camel-case arguments", async () => {
    const calls: Array<{ command: CommandName; args: Record<string, unknown> | undefined }> = [];
    const api = createFormApi(<T,>(command: CommandName, args?: Record<string, unknown>) => {
      calls.push({ command, args });
      return Promise.resolve((command === "form_current" ? fixtures.form.open : undefined) as T);
    });
    await api.ready();
    const form = await api.current();
    await api.laidOut(2, 312.5);
    await api.painted(2, 1234.5);
    await api.submit(2, { Size: "S" });
    await api.cancel(2);
    await api.abandon(2, { Size: "M" });
    expect(form?.label).toBe("Back soon");
    expect(calls).toEqual([
      { command: "form_ready", args: undefined },
      { command: "form_current", args: undefined },
      { command: "form_laid_out", args: { session: 2, height: 312.5 } },
      { command: "form_painted", args: { session: 2, paintedAtMs: 1234.5 } },
      { command: "form_submit", args: { session: 2, values: { Size: "S" } } },
      { command: "form_cancel", args: { session: 2 } },
      { command: "form_abandon", args: { session: 2, values: { Size: "M" } } },
    ]);
    // Every form command is registered, and reachable from this API.
    expect(new Set(calls.map((c) => c.command))).toEqual(
      new Set(COMMANDS.filter((c) => c.startsWith("form_"))),
    );
  });

  it("answer null when no form is open, or the answer is not a form", async () => {
    const none = createFormApi(<T,>() => Promise.resolve(null as T));
    expect(await none.current()).toBeNull();
    const junk = createFormApi(<T,>() => Promise.resolve({ session: "2" } as T));
    expect(await junk.current()).toBeNull();
  });

  it("name the panel's event", () => {
    expect(FORM_EVENT).toBe("iamb:form");
  });
});

describe("payloads written by the Rust side (contract.fixtures.json, form)", () => {
  it("are accepted by the guards", () => {
    expect(isFormOpen(fixtures.form.open)).toBe(true);
    expect(fixtures.form.open.lastValues).toEqual({ Name: "Ada" });
    expect(isFormOpen(fixtures.form.openWithoutValues)).toBe(true);
    expect(fixtures.form.openWithoutValues.app).toBe("Mail");
    expect(fixtures.form.events).toHaveLength(4);
    for (const event of fixtures.form.events) expect(isFormEvent(event)).toBe(true);
    expect(isFocusLost(fixtures.form.focusLost)).toBe(true);
    expect(fixtures.form.focusLost.code).toBe(FOCUS_LOST);
  });

  it("are rejected when their shape changes", () => {
    const open = fixtures.form.open;
    expect(isFormOpen({ ...open, lastValues: { Name: 1 } })).toBe(false);
    expect(isFormOpen({ ...open, fields: [{ name: "x", kind: "date" }] })).toBe(false);
    expect(isFormOpen({ ...open, app: 3 })).toBe(false);
    expect(isFormEvent({ type: "open" })).toBe(false);
    expect(isFormEvent({ type: "submitted", session: 1 })).toBe(false);
    expect(isFocusLost({ code: "timeout", message: "x" })).toBe(false);
  });
});
