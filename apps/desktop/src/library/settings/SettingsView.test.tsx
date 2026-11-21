// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { SETTING_KEYS, type SettingKey } from "./api";
import { PRIVACY_STATEMENTS } from "./PrivacySection";
import { SettingsView } from "./SettingsView";
import { Failure } from "../testkit";
import { type Rig, emitEvent, rig } from "./testkit";

afterEach(() => {
  delete document.documentElement.dataset.theme;
});

async function open(r: Rig, section?: string) {
  const user = userEvent.setup();
  render(<SettingsView env={r.env} />);
  await screen.findByRole("heading", { name: "General" });
  if (section) await user.click(screen.getByRole("button", { name: section }));
  return user;
}

const stored = (r: Rig, key: string) => r.settings.stored(key);

// One case per setting: set it through the page and see what the service was given. The service's
// own side (it persists, also over a restart) is `tests/settings.rs`, whose test lists the same
// keys; this table is typed over every key, and the test after it fails when one has no case.
const CASES: Record<SettingKey, (r: Rig) => Promise<void>> = {
  delimiters: async (r) => {
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("checkbox", { name: /^Punctuation/ }));
    expect(stored(r, "delimiters")).toBe(" )]}\"'");
    // Back to the default set: the setting is removed, not stored as a copy of the default.
    await user.click(screen.getByRole("checkbox", { name: /^Punctuation/ }));
    expect(stored(r, "delimiters")).toBeUndefined();
    expect(r.settings.puts.map((p) => p.value)).toEqual([" )]}\"'", null]);
  },
  date_format: async (r) => {
    const user = await open(r, "Dates");
    await user.selectOptions(screen.getByRole("combobox", { name: "Date format" }), "%d/%m/%Y");
    expect(stored(r, "date_format")).toBe("%d/%m/%Y");
  },
  time_format: async (r) => {
    const user = await open(r, "Dates");
    await user.selectOptions(screen.getByRole("combobox", { name: "Time format" }), "%-I:%M %p");
    expect(stored(r, "time_format")).toBe("%-I:%M %p");
  },
  locale: async (r) => {
    const user = await open(r, "Dates");
    await user.selectOptions(screen.getByRole("combobox", { name: "Month and day names in" }), "de");
    expect(stored(r, "locale")).toBe("de");
  },
  palette_hotkey: async (r) => {
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    await user.keyboard("{Control>}{Shift>}k{/Shift}{/Control}");
    await waitFor(() => expect(stored(r, "palette_hotkey")).toBe("Ctrl+Shift+K"));
  },
  reopen_form_hotkey: async (r) => {
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Fill in the last form again/ }));
    await user.keyboard("{Control>}{Alt>}f{/Alt}{/Control}");
    await waitFor(() => expect(stored(r, "reopen_form_hotkey")).toBe("Ctrl+Alt+F"));
  },
  pause_hotkey: async (r) => {
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Pause or resume/ }));
    await user.keyboard("{Control>}{Alt>}p{/Alt}{/Control}");
    await waitFor(() => expect(stored(r, "pause_hotkey")).toBe("Ctrl+Alt+P"));
  },
  injection_overrides: async (r) => {
    const user = await open(r, "Apps");
    const group = screen.getByRole("group", { name: "If snippets come out wrong in an app" });
    await user.type(within(group).getByLabelText("Program file name"), "chat.exe");
    await user.click(within(group).getByRole("button", { name: "Add app" }));
    await waitFor(() => expect(stored(r, "injection_overrides")).toBe('{"apps":{"chat.exe":{}}}'));
    await user.selectOptions(await within(group).findByLabelText("Send the text"), "paste");
    await user.selectOptions(within(group).getByLabelText("Line breaks as"), "shift+enter");
    await user.type(within(group).getByLabelText("Slow down by"), "5{Enter}");
    await waitFor(() =>
      expect(JSON.parse(stored(r, "injection_overrides") ?? "{}")).toEqual({
        apps: { "chat.exe": { method: "paste", line_break: "shift+enter", key_delay_ms: 5 } },
      }),
    );
  },
  // Written by onboarding, not shown here: Settings must read it and keep it as it is.
  "ui.onboarding_done": async (r) => {
    r.settings.values.set("ui.onboarding_done", "1");
    const user = await open(r, "Privacy");
    await user.click(screen.getByRole("switch", { name: "Send crash reports" }));
    expect(stored(r, "ui.onboarding_done")).toBe("1");
    expect(r.settings.puts.map((p) => p.key)).toEqual(["ui.crash_reports"]);
  },
  "ui.theme": async (r) => {
    const user = await open(r);
    await user.click(screen.getByRole("radio", { name: "Dark" }));
    expect(stored(r, "ui.theme")).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    await user.click(screen.getByRole("radio", { name: "Follow the system" }));
    expect(stored(r, "ui.theme")).toBeUndefined();
    expect(document.documentElement.dataset.theme).toBeUndefined();
  },
  "ui.tray_icon": async (r) => {
    const user = await open(r);
    const sw = screen.getByRole("switch", { name: "Show the Iamb icon in the notification area" });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    await user.click(sw);
    expect(stored(r, "ui.tray_icon")).toBe("off");
    await user.click(sw);
    expect(stored(r, "ui.tray_icon")).toBeUndefined();
  },
  "ui.new_trigger": async (r) => {
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("radio", { name: "Immediately" }));
    expect(stored(r, "ui.new_trigger")).toBe("immediate");
  },
  "ui.new_case_mode": async (r) => {
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("radio", { name: "Exactly as written" }));
    expect(stored(r, "ui.new_case_mode")).toBe("exact");
  },
  "ui.crash_reports": async (r) => {
    const user = await open(r, "Privacy");
    const sw = screen.getByRole("switch", { name: "Send crash reports" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    await user.click(sw);
    expect(stored(r, "ui.crash_reports")).toBe("on");
  },
};

describe("each setting is written where the service reads it", () => {
  for (const key of SETTING_KEYS) {
    it(`writes ${key}`, async () => {
      await CASES[key](rig());
    });
  }

  it("has a case for every key", () => {
    expect(Object.keys(CASES).sort()).toEqual([...SETTING_KEYS].sort());
  });
});

describe("the page", () => {
  it("shows what is stored", async () => {
    const r = rig({
      delimiters: ` .,;:!?)]}"'\n`,
      date_format: "%d.%m.%Y",
      locale: "fr-CA",
      "ui.theme": "light",
      "ui.new_trigger": "immediate",
      palette_hotkey: "Ctrl+Alt+K",
    });
    const user = await open(r);
    expect(screen.getByRole("radio", { name: "Light" })).toHaveProperty("checked", true);
    await user.click(screen.getByRole("button", { name: "Typing" }));
    expect(screen.getByRole("checkbox", { name: "Enter" })).toHaveProperty("checked", true);
    expect(screen.getByRole("checkbox", { name: "Tab" })).toHaveProperty("checked", false);
    expect(screen.getByRole("radio", { name: "Immediately" })).toHaveProperty("checked", true);
    await user.click(screen.getByRole("button", { name: "Dates" }));
    expect(screen.getByRole("combobox", { name: "Month and day names in" })).toHaveProperty("value", "fr");
    expect(screen.getByRole("combobox", { name: "Date format" })).toHaveProperty("value", "%d.%m.%Y");
    await user.click(screen.getByRole("button", { name: "Shortcuts" }));
    expect(screen.getByRole("button", { name: /^Search snippets/ }).textContent).toContain("Ctrl");
  });

  it("applies the stored appearance and asks nothing more", async () => {
    await open(rig({ "ui.theme": "dark" }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
  });

  it("has a list of sections and moves between them with the keyboard", async () => {
    const user = await open(rig());
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "General",
      "Typing",
      "Dates",
      "Shortcuts",
      "Apps",
      "Privacy",
      "About",
    ]);
    expect(within(nav).getByRole("button", { name: "General" }).getAttribute("aria-current")).toBe("page");
    within(nav).getByRole("button", { name: "Typing" }).focus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("heading", { name: "Typing" })).toBeTruthy();
  });

  it("can be opened at a section", async () => {
    const r = rig();
    render(<SettingsView env={r.env} section="apps" />);
    expect(await screen.findByRole("heading", { name: "Apps" })).toBeTruthy();
  });

  it("says it is not available when the app gives it no commands", () => {
    const r = rig({}, { settings: undefined });
    render(<SettingsView env={r.env} />);
    expect(screen.getByText("This view is not available yet.")).toBeTruthy();
  });

  it("reads the settings again when they change elsewhere", async () => {
    const r = rig();
    await open(r, "Typing");
    expect(screen.getByRole("radio", { name: "Immediately" })).toHaveProperty("checked", false);
    r.settings.values.set("ui.new_trigger", "immediate");
    emitEvent(r.library, { type: "settings_changed" });
    await waitFor(() => expect(screen.getByRole("radio", { name: "Immediately" })).toHaveProperty("checked", true));
  });

  it("cannot change anything while Iamb is not running", async () => {
    const r = rig({}, { connection: "disconnected" });
    render(<SettingsView env={r.env} />);
    expect(await screen.findByText("Reading the settings…")).toBeTruthy();
    expect(r.settings.calls).toEqual([]);
  });

  it("says when the first read fails, and tries again", async () => {
    const r = rig();
    r.settings.setDown("unavailable");
    render(<SettingsView env={r.env} />);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Iamb is busy. Try again in a moment.");
    r.settings.setDown(null);
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "General" })).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });
});

describe("General", () => {
  it("pauses and resumes through the service's pause", async () => {
    const r = rig();
    const calls: boolean[] = [];
    r.env.api.setPaused = async (paused) => {
      calls.push(paused);
      return { connection: "connected", service: null };
    };
    const user = await open(r);
    const sw = screen.getByRole("switch", { name: "Expand snippets as I type" });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    await user.click(sw);
    expect(calls).toEqual([true]);
  });

  it("shows the paused state", async () => {
    const r = rig({}, { paused: true });
    await open(r);
    expect(screen.getByRole("switch", { name: "Expand snippets as I type" }).getAttribute("aria-checked")).toBe("false");
  });

  it("names the menu bar and the Dock on macOS", async () => {
    await open(rig({}, { mac: true }));
    const sw = screen.getByRole("switch", { name: "Show the Iamb icon in the menu bar" });
    expect(sw.getAttribute("aria-describedby")).toBeTruthy();
    expect(screen.getByText("Iamb keeps running without it. Open it from the Dock.")).toBeTruthy();
  });
});

describe("Typing", () => {
  it("asks before Enter expands, and does nothing on Cancel", async () => {
    const r = rig();
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("checkbox", { name: "Enter" }));
    const dialog = screen.getByRole("dialog", { name: "Expand on Enter?" });
    expect(dialog.textContent).toContain(
      "Apps act on Enter before Iamb can type: a chat sends the message, a dialog closes, a form moves on.",
    );
    expect(dialog.textContent).toContain("Turn it on only if you use Iamb in plain documents.");
    // Cancel has the focus first: Enter must not turn it on by accident.
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Cancel" }));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(r.settings.puts).toEqual([]);
    expect(screen.getByRole("checkbox", { name: "Enter" })).toHaveProperty("checked", false);
  });

  it("turns Enter on when asked to anyway, and keeps a warning in view", async () => {
    const r = rig();
    const user = await open(r, "Typing");
    expect(screen.queryByText(/also expands/)).toBeNull();
    await user.click(screen.getByRole("checkbox", { name: "Enter" }));
    await user.click(screen.getByRole("button", { name: "Turn on anyway" }));
    expect(stored(r, "delimiters")).toBe(` .,;:!?)]}"'\n`);
    expect(screen.getByText("Enter also expands. Snippets may land in the wrong place in chats, forms and dialogs.")).toBeTruthy();
    // Turning it off needs no question.
    await user.click(screen.getByRole("checkbox", { name: "Enter" }));
    expect(stored(r, "delimiters")).toBeUndefined();
    expect(screen.queryByText(/also expands/)).toBeNull();
  });

  it("asks about Tab with its own words", async () => {
    const r = rig();
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("checkbox", { name: "Tab" }));
    const dialog = screen.getByRole("dialog", { name: "Expand on Tab?" });
    expect(dialog.textContent).toContain("Tab moves to the next field of a form.");
    await user.click(within(dialog).getByRole("button", { name: "Turn on anyway" }));
    expect(stored(r, "delimiters")).toContain("\t");
    await user.click(screen.getByRole("checkbox", { name: "Enter" }));
    await user.click(screen.getByRole("button", { name: "Turn on anyway" }));
    expect(screen.getByText(/^Enter and Tab also expand\./)).toBeTruthy();
  });

  it("closes the question with Esc", async () => {
    const r = rig();
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("checkbox", { name: "Tab" }));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(r.settings.puts).toEqual([]);
  });

  it("always expands after a space, which cannot be turned off", async () => {
    await open(rig(), "Typing");
    const space = screen.getByRole("checkbox", { name: "A space" });
    expect(space).toHaveProperty("checked", true);
    expect(space).toHaveProperty("disabled", true);
  });

  it("shows a partly chosen group as mixed", async () => {
    await open(rig({ delimiters: " .,", "ui.new_case_mode": "adaptive" }), "Typing");
    expect(screen.getByRole("checkbox", { name: /^Punctuation/ })).toHaveProperty("indeterminate", true);
    expect(screen.getByRole("radio", { name: "Ignore, and follow mine" })).toHaveProperty("checked", true);
  });

  it("keeps a character the boxes do not know when a group changes", async () => {
    const r = rig({ delimiters: " .,;:!?)]}\"'-" });
    const user = await open(r, "Typing");
    await user.click(screen.getByRole("checkbox", { name: /^Closing brackets/ }));
    expect(stored(r, "delimiters")).toBe(" .,;:!?-");
  });
});

describe("Dates", () => {
  it("lists formats as today's date and offers a custom one", async () => {
    const r = rig();
    const user = await open(r, "Dates");
    const select = screen.getByRole("combobox", { name: "Date format" });
    const today = new Date();
    const iso = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
    expect(within(select).getByRole("option", { name: iso })).toBeTruthy();
    await user.selectOptions(select, "custom");
    const field = screen.getByRole("textbox", { name: "Date format, custom format" });
    await user.clear(field);
    await user.type(field, "%A %-d %B{Enter}");
    expect(stored(r, "date_format")).toBe("%A %-d %B");
    expect(screen.getByText(/^Looks like /).textContent).toMatch(/^Looks like \w+ \d{1,2} \w+$/);
  });

  it("says which code it does not know, and stores nothing", async () => {
    const r = rig();
    const user = await open(r, "Dates");
    await user.selectOptions(screen.getByRole("combobox", { name: "Date format" }), "custom");
    const field = screen.getByRole("textbox", { name: "Date format, custom format" });
    await user.clear(field);
    await user.type(field, "%Y %Q{Enter}");
    expect(screen.getByRole("alert").textContent).toBe("Iamb does not know “%Q”. See the list of date codes.");
    expect(stored(r, "date_format")).toBeUndefined();
    expect(screen.getByText("Date codes")).toBeTruthy();
  });

  it("shows the service's refusal under the setting", async () => {
    const r = rig();
    const user = await open(r, "Dates");
    await user.selectOptions(screen.getByRole("combobox", { name: "Time format" }), "custom");
    const field = screen.getByRole("textbox", { name: "Time format, custom format" });
    // A format the page accepts but the service refuses (the service decides).
    r.settings.api.putSetting = async () => {
      throw Object.assign(new Error("x"), { code: "invalid", message: "invalid time_format setting" });
    };
    await user.clear(field);
    await user.type(field, "%H h{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Iamb could not accept that value. invalid time_format setting",
    );
  });

  it("shows examples in the chosen language", async () => {
    await open(rig({ locale: "es", date_format: "%A" }), "Dates");
    const names = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    const select = screen.getByRole("combobox", { name: "Date format" });
    // The stored format is not in the list, so the field shows it as a custom one.
    expect(select).toHaveProperty("value", "custom");
    const field = screen.getByRole("textbox", { name: "Date format, custom format" });
    expect(field).toHaveProperty("value", "%A");
    expect(names.some((n) => screen.getByText(/^Looks like /).textContent?.includes(n))).toBe(true);
  });
});

describe("Shortcuts", () => {
  it("has the two shortcuts that are unset by default, each cleared with Backspace", async () => {
    const r = rig({ pause_hotkey: "Ctrl+Alt+P" });
    const user = await open(r, "Shortcuts");
    const again = screen.getByRole("button", { name: /^Fill in the last form again/ });
    expect(again.textContent).toContain("Not set");
    expect(again.getAttribute("aria-describedby")).toBeTruthy();
    const pause = screen.getByRole("button", { name: /^Pause or resume/ });
    expect(pause.textContent).toContain("Ctrl");
    // Only the search shortcut has a default to offer.
    expect(screen.queryAllByRole("button", { name: /^Use / })).toHaveLength(1);
    await user.click(pause);
    await user.keyboard("{Backspace}");
    await waitFor(() => expect(stored(r, "pause_hotkey")).toBeUndefined());
  });

  it("says when another app already uses a shortcut", async () => {
    const r = rig();
    r.settings.api.putSetting = async () => {
      throw new Failure("conflict", "the system refused this shortcut");
    };
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Pause or resume/ }));
    await user.keyboard("{Control>}{Alt>}p{/Alt}{/Control}");
    expect((await screen.findByRole("alert")).textContent).toBe("Another app already uses this shortcut");
  });

  it("says Not set before there is a shortcut and offers the default", async () => {
    const r = rig();
    const user = await open(r, "Shortcuts");
    expect(screen.getByRole("button", { name: /^Search snippets/ }).textContent).toContain("Not set");
    await user.click(screen.getByRole("button", { name: "Use Ctrl+Shift+Space" }));
    expect(stored(r, "palette_hotkey")).toBe("Ctrl+Shift+Space");
    expect(screen.queryByRole("button", { name: /^Use / })).toBeNull();
  });

  it("says Press the keys while it listens, and Esc leaves it as it was", async () => {
    const r = rig({ palette_hotkey: "Ctrl+Shift+Space" });
    const user = await open(r, "Shortcuts");
    const field = screen.getByRole("button", { name: /^Search snippets/ });
    await user.click(field);
    expect(field.textContent).toContain("Press the keys");
    await user.keyboard("{Escape}");
    expect(document.activeElement).not.toBe(field);
    expect(stored(r, "palette_hotkey")).toBe("Ctrl+Shift+Space");
    expect(field.textContent).toContain("Space");
  });

  it("clears with Backspace", async () => {
    const r = rig({ palette_hotkey: "Ctrl+Shift+Space" });
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    await user.keyboard("{Backspace}");
    await waitFor(() => expect(stored(r, "palette_hotkey")).toBeUndefined());
    expect(r.settings.puts.at(-1)).toEqual({ key: "palette_hotkey", value: null });
  });

  it("lets Tab move on, so the keyboard is not trapped", async () => {
    const r = rig({ palette_hotkey: "Ctrl+Shift+Space" });
    const user = await open(r, "Shortcuts");
    const field = screen.getByRole("button", { name: /^Search snippets/ });
    await user.click(field);
    await user.tab();
    expect(document.activeElement).not.toBe(field);
    expect(r.settings.puts).toEqual([]);
  });

  it("refuses a chord without Ctrl, Alt or the Windows key", async () => {
    const r = rig();
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    await user.keyboard("k");
    expect((await screen.findByRole("alert")).textContent).toBe("Add Ctrl, Alt or the Windows key");
    expect(r.settings.puts).toEqual([]);
  });

  it("says Another app already uses this shortcut, and keeps the old one", async () => {
    const r = rig({ palette_hotkey: "Ctrl+Shift+Space" });
    r.settings.takeChord("Ctrl+Alt+J");
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    await user.keyboard("{Control>}{Alt>}j{/Alt}{/Control}");
    expect((await screen.findByRole("alert")).textContent).toBe("Another app already uses this shortcut");
    expect(stored(r, "palette_hotkey")).toBe("Ctrl+Shift+Space");
  });

  it("says when the service cannot use the chord", async () => {
    const r = rig();
    const user = await open(r, "Shortcuts");
    r.settings.api.putSetting = async () => {
      throw Object.assign(new Error("x"), { code: "invalid", message: "not a valid chord" });
    };
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "F9", code: "F9" });
    expect((await screen.findByRole("alert")).textContent).toBe("Iamb cannot use that shortcut. Try another one.");
  });

  it("names Control, Option and Command on macOS", async () => {
    const r = rig({}, { mac: true });
    const user = await open(r, "Shortcuts");
    await user.click(screen.getByRole("button", { name: /^Search snippets/ }));
    await user.keyboard("k");
    expect((await screen.findByRole("alert")).textContent).toBe("Add Control, Option or Command");
    expect(screen.getByRole("button", { name: "Use ⌃⇧Space" })).toBeTruthy();
  });
});

describe("Apps", () => {
  const bank = [{ appId: "mybank.exe", displayName: "My bank", createdAt: 1 }];

  it("lists the excluded apps with their names", async () => {
    await open(rig({}, {}, { exclusions: bank }), "Apps");
    const list = screen.getByRole("list", { name: "Excluded apps" });
    expect(within(list).getByText("My bank")).toBeTruthy();
    expect(within(list).getByText("mybank.exe")).toBeTruthy();
    expect(
      screen.getByText(
        "Iamb does not store keys typed in these apps. Add password managers, remote desktops and terminals where you type passwords.",
      ),
    ).toBeTruthy();
  });

  it("adds an app by its id, and takes the id as the service spells it", async () => {
    const r = rig();
    const user = await open(r, "Apps");
    expect(screen.getByText("No app is excluded.")).toBeTruthy();
    const group = screen.getByRole("group", { name: "Never expand in these apps" });
    await user.type(within(group).getByLabelText("Program file name"), "  MyBank.EXE ");
    await user.type(within(group).getByLabelText(/^Name/), "My bank");
    await user.click(within(group).getByRole("button", { name: "Add app" }));
    const list = await screen.findByRole("list", { name: "Excluded apps" });
    expect(within(list).getByText("My bank")).toBeTruthy();
    expect(r.settings.exclusions.map((x) => x.appId)).toEqual(["mybank.exe"]);
    // The fields are empty again.
    expect(within(group).getByLabelText("Program file name")).toHaveProperty("value", "");
  });

  it("uses the id as the name when none is given", async () => {
    const r = rig();
    const user = await open(r, "Apps");
    const group = screen.getByRole("group", { name: "Never expand in these apps" });
    await user.type(within(group).getByLabelText("Program file name"), "putty.exe{Enter}");
    await screen.findByRole("list", { name: "Excluded apps" });
    expect(r.settings.exclusions[0]?.displayName).toBe("putty.exe");
  });

  it("says what is wrong with an id, and what a bundle identifier is on macOS", async () => {
    const r = rig({}, { mac: true });
    const user = await open(r, "Apps");
    const group = screen.getByRole("group", { name: "Never expand in these apps" });
    await user.click(within(group).getByRole("button", { name: "Add app" }));
    expect(within(group).getByRole("alert").textContent).toBe(
      "Write the app's bundle identifier, for example com.example.bank.",
    );
    await user.type(within(group).getByLabelText("Bundle identifier"), "/Applications/Bank.app");
    await user.click(within(group).getByRole("button", { name: "Add app" }));
    expect((await within(group).findByRole("alert")).textContent).toContain("Iamb could not accept that value.");
    expect(r.settings.exclusions).toEqual([]);
  });

  it("removes an app", async () => {
    const r = rig({}, {}, { exclusions: bank });
    const user = await open(r, "Apps");
    await user.click(screen.getByRole("button", { name: "Remove My bank" }));
    await waitFor(() => expect(screen.getByText("No app is excluded.")).toBeTruthy());
    expect(r.settings.exclusions).toEqual([]);
  });

  it("lets an app have its own way of typing, and keeps what else the document holds", async () => {
    const doc = '{"defaults":{"paste_threshold":9},"apps":{"chat.exe":{"method":"paste","paste_keys":"ctrl+v"}}}';
    const r = rig({ injection_overrides: doc });
    const user = await open(r, "Apps");
    const list = screen.getByRole("list", { name: "Apps with their own settings" });
    expect(within(list).getByLabelText("Send the text")).toHaveProperty("value", "paste");
    await user.selectOptions(within(list).getByLabelText("Line breaks as"), "enter");
    await waitFor(() =>
      expect(JSON.parse(stored(r, "injection_overrides") ?? "")).toEqual({
        defaults: { paste_threshold: 9 },
        apps: { "chat.exe": { method: "paste", paste_keys: "ctrl+v", line_break: "enter" } },
      }),
    );
    // "Let Iamb decide" removes the field.
    await user.selectOptions(within(list).getByLabelText("Send the text"), "decide");
    await waitFor(() => expect(stored(r, "injection_overrides")).not.toContain('"method"'));
    // Removing the app leaves the rest of the document.
    await user.click(within(list).getByRole("button", { name: "Remove the settings for chat.exe" }));
    await waitFor(() => expect(stored(r, "injection_overrides")).toBe('{"defaults":{"paste_threshold":9}}'));
  });

  it("removes the setting when the last app goes", async () => {
    const r = rig({ injection_overrides: '{"apps":{"chat.exe":{"method":"keys"}}}' });
    const user = await open(r, "Apps");
    await user.click(screen.getByRole("button", { name: "Remove the settings for chat.exe" }));
    await waitFor(() => expect(stored(r, "injection_overrides")).toBeUndefined());
  });

  it("checks the pause between keys", async () => {
    const r = rig({ injection_overrides: '{"apps":{"chat.exe":{}}}' });
    const user = await open(r, "Apps");
    const delay = screen.getByLabelText("Slow down by");
    await user.type(delay, "99999{Enter}");
    expect(screen.getByRole("alert").textContent).toBe("Use a whole number from 0 to 10000.");
    expect(r.settings.puts).toEqual([]);
    await user.clear(delay);
    await user.type(delay, "40{Enter}");
    await waitFor(() => expect(stored(r, "injection_overrides")).toBe('{"apps":{"chat.exe":{"key_delay_ms":40}}}'));
  });

  it("does not add an app twice", async () => {
    const r = rig({ injection_overrides: '{"apps":{"chat.exe":{}}}' });
    const user = await open(r, "Apps");
    const group = screen.getByRole("group", { name: "If snippets come out wrong in an app" });
    await user.type(within(group).getByLabelText("Program file name"), "Chat{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe("That app is in the list already.");
  });

  it("does not edit a document someone wrote by hand and broke", async () => {
    const r = rig({ injection_overrides: "{oops" });
    await open(r, "Apps");
    expect(screen.getByText("These settings were written by hand and Iamb cannot edit them here.")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Apps with their own settings" })).toBeNull();
    expect(stored(r, "injection_overrides")).toBe("{oops");
  });
});

describe("Privacy and About", () => {
  it("states what Iamb keeps, word for word", async () => {
    await open(rig(), "Privacy");
    for (const s of PRIVACY_STATEMENTS) expect(screen.getByText(s)).toBeTruthy();
    expect(PRIVACY_STATEMENTS).toEqual([
      "Keys you type are held in memory for a moment and never saved or sent.",
      "How often you use a snippet is counted on this computer only.",
      "The log of why something did not expand lives in memory and is gone when Iamb restarts.",
    ]);
    expect(
      screen.getByText("A report says where Iamb failed. It never contains what you typed, your snippets or your clipboard."),
    ).toBeTruthy();
  });

  it("names the licence and the source", async () => {
    await open(rig(), "About");
    expect(screen.getByText("AGPL-3.0-or-later")).toBeTruthy();
    expect(screen.getByText(/github\.com\/gotolisto\/iamb/)).toBeTruthy();
  });

  it("keeps what is typed into a field in the field until it is added", async () => {
    const r = rig();
    const user = await open(r, "Apps");
    await user.type(screen.getAllByLabelText("Program file name")[0] as HTMLElement, "zqxcanary.exe");
    expect(document.title).not.toContain("zqxcanary");
    expect(window.localStorage.length).toBe(0);
    expect(r.settings.puts).toEqual([]);
  });
});

describe("the usage reset", () => {
  it("asks first, and forgets the counts only when confirmed", async () => {
    const r = rig();
    const user = await open(r, "Privacy");
    r.library.setUsage("x", 3, 1);
    await user.click(screen.getByRole("button", { name: "Reset usage counts…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Reset usage counts?" });
    expect(dialog.textContent).toContain("on this computer");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(r.library.calls).not.toContain("resetUsage");
    expect(r.library.usage.size).toBe(1);

    await user.click(screen.getByRole("button", { name: "Reset usage counts…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Reset counts" }));
    expect(await screen.findByText("Usage counts were reset.")).toBeTruthy();
    expect(r.library.usage.size).toBe(0);
  });

  it("says so when the service does not answer", async () => {
    const r = rig();
    const user = await open(r, "Privacy");
    r.library.setDown("disconnected");
    await user.click(screen.getByRole("button", { name: "Reset usage counts…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Reset counts" }));
    expect(await screen.findByText(/Iamb is not running/)).toBeTruthy();
    expect(screen.queryByText("Usage counts were reset.")).toBeNull();
  });
});
