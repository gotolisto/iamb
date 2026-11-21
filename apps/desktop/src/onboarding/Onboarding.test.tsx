// @vitest-environment jsdom
import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import {
  NOTHING_GRANTED,
  NOT_REQUIRED,
  SAMPLE_EMAIL,
  createFake,
  field,
  heading,
  onStep,
  personTypes,
  renderOnboarding,
  rig,
  serviceTypes,
} from "./testkit";

const button = (name: string | RegExp) => screen.getByRole("button", { name });
const progress = () => document.querySelector(".ob-progress")?.textContent ?? "";

/** Opens the first snippet step on a Windows-like machine. */
async function toSnippet(options: Parameters<typeof rig>[0] = {}) {
  const r = rig(options);
  await renderOnboarding(r);
  await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
  await onStep(/Make your first snippet/);
  return r;
}

/** What the field was told, consecutive repeats of one answer counted once (it is renewed). */
const told = (r: { testField: boolean[] }) => r.testField.filter((on, i) => i === 0 || on !== r.testField[i - 1]);

/** Saves the starter snippet and arrives at the Try step. */
async function toTry(options: Parameters<typeof rig>[0] = {}) {
  const r = await toSnippet(options);
  const user = userEvent.setup();
  await user.type(screen.getByLabelText("Iamb types"), SAMPLE_EMAIL);
  await user.click(button(/Save and try it/));
  await onStep(/Try it here/);
  return r;
}

describe("Step 1: Welcome", () => {
  it("shows the copy, focuses Get started and goes on with Enter", async () => {
    const r = rig();
    await renderOnboarding(r);
    expect(heading()).toMatch(/Type a few letters\. Get the whole thing\./);
    expect(screen.getByText(/Save the text you type again and again as a snippet/)).toBeTruthy();
    expect(screen.getByRole("img", { name: "Example: typing ;em gives ines@brindlewood.example" })).toBeTruthy();
    expect(screen.getByText("What you type stays on this computer")).toBeTruthy();
    expect(screen.getByText(/It never saves or sends what you type\./)).toBeTruthy();
    expect(screen.getByText("It stays out of password fields")).toBeTruthy();
    expect(screen.getByText("And out of any app you tell it to leave alone.")).toBeTruthy();
    const start = await screen.findByRole("button", { name: /Get started/ });
    await waitFor(() => expect(document.activeElement).toBe(start));
    expect(progress()).toContain("Step 1 of 4");
    await userEvent.setup().keyboard("{Enter}");
    await onStep(/Make your first snippet/);
  });

  it("counts five steps on macOS while a permission is missing, four when both are granted", async () => {
    const missing = rig({ mac: true });
    const first = await renderOnboarding(missing);
    expect(progress()).toContain("Step 1 of 5");
    first.unmount();

    const granted = rig({ mac: true, permissions: { accessibility: "granted", inputMonitoring: "granted" } });
    await renderOnboarding(granted);
    await waitFor(() => expect(progress()).toContain("Step 1 of 4"));
    await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
    await onStep(/Make your first snippet/);
    expect(progress()).toContain("Step 2 of 4");
  });

  it("says the background service is not running and offers to start it again", async () => {
    const r = rig({ service: null });
    await renderOnboarding(r);
    expect(await screen.findByText(/Iamb’s background service is not running\./)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Get started/ })).toBeNull();
    const user = userEvent.setup();
    await user.click(button("Start it again"));
    expect(r.restarts).toBe(1);
    // The step returns to normal when the service answers.
    r.setService({});
    expect(await screen.findByRole("button", { name: /Get started/ })).toBeTruthy();
    expect(screen.queryByText(/background service is not running/)).toBeNull();
  });

  it("tells what to do when this build cannot start the service", async () => {
    const r = rig({ service: null });
    r.failRestart();
    await renderOnboarding(r);
    await userEvent.setup().click(await screen.findByRole("button", { name: "Start it again" }));
    expect(await screen.findByText(/Quit Iamb from its menu and open it again\./)).toBeTruthy();
  });
});

describe("Step 2: Permissions (macOS)", () => {
  async function toPermissions() {
    const r = rig({ mac: true });
    await renderOnboarding(r);
    await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
    await onStep(/Let Iamb see and type keys/);
    return r;
  }

  it("lists both permissions, not granted, with Continue disabled", async () => {
    await toPermissions();
    expect(progress()).toContain("Step 2 of 5");
    expect(screen.getByText(/macOS asks before any app may do this\. Turn on Iamb in two lists\. This window updates by itself\./)).toBeTruthy();
    expect(screen.getByText("Lets Iamb notice when you type an abbreviation.")).toBeTruthy();
    expect(screen.getByText("Lets Iamb type your snippet for you, and recognise password fields so it can stay out of them.")).toBeTruthy();
    expect(screen.getAllByText("Not granted yet")).toHaveLength(2);
    expect(screen.getByText(/turn on the switch next to/)).toBeTruthy();
    expect((button("Continue") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Skip for now" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy();
  });

  it("opens the settings page of each permission, the first missing one by Enter", async () => {
    const r = await toPermissions();
    const user = userEvent.setup();
    const rows = screen.getAllByRole("button", { name: /Open settings/ });
    await waitFor(() => expect(document.activeElement).toBe(rows[0]));
    await user.keyboard("{Enter}");
    expect(r.opened).toEqual(["input_monitoring"]);
    await user.click(rows[1] as HTMLElement);
    expect(r.opened).toEqual(["input_monitoring", "accessibility"]);
    // Enter with nothing in particular focused opens the first one still missing.
    (document.activeElement as HTMLElement | null)?.blur();
    fireEvent.keyDown(document.querySelector(".ob-step") as HTMLElement, { key: "Enter" });
    expect(r.opened).toHaveLength(3);
    expect(r.opened[2]).toBe("input_monitoring");
  });

  it("updates by itself when a permission is granted, and moves the keyboard to Continue when both are", async () => {
    const r = await toPermissions();
    r.grant("input_monitoring");
    expect(await screen.findByText("Granted")).toBeTruthy();
    expect(screen.getAllByText("Not granted yet")).toHaveLength(1);
    // The granted row has no button; the screen reader hears "Input Monitoring: granted".
    expect(screen.getAllByRole("button", { name: /Open settings/ })).toHaveLength(1);
    const status = screen.getAllByRole("status").find((s) => /Input Monitoring/.test(s.textContent ?? ""));
    expect(status?.textContent).toBe("Input Monitoring: Granted");
    expect((button("Continue") as HTMLButtonElement).disabled).toBe(true);

    r.grantAll();
    expect(await screen.findByText("Both are on. You can change them at any time in System Settings.")).toBeTruthy();
    const next = button(/Continue/) as HTMLButtonElement;
    expect(next.disabled).toBe(false);
    await waitFor(() => expect(document.activeElement).toBe(next));
    expect(screen.queryByText(/turn on the switch next to/)).toBeNull();
    await userEvent.setup().keyboard("{Enter}");
    await onStep(/Make your first snippet/);
    expect(progress()).toContain("Step 3 of 5");
  });

  it("asks for a restart when both grants are there and the listener is still off", async () => {
    const r = await toPermissions();
    r.setService({ listener: false });
    r.grantAll();
    expect(await screen.findByText(/Iamb needs to restart its helper to use the new permissions\./)).toBeTruthy();
    await userEvent.setup().click(button("Restart"));
    expect(r.restarts).toBe(1);
    r.setService({ listener: true });
    await waitFor(() => expect(screen.queryByText(/needs to restart its helper/)).toBeNull());
  });

  it("skips to the first snippet and then says Iamb cannot expand yet", async () => {
    const r = await toPermissions();
    const user = userEvent.setup();
    await user.click(button("Skip for now"));
    await onStep(/Make your first snippet/);
    await user.type(screen.getByLabelText("Iamb types"), SAMPLE_EMAIL);
    await user.click(button(/Save and try it/));
    await onStep(/Iamb cannot expand yet/);
    expect(screen.queryByLabelText("Test field")).toBeNull();
    // Nothing is declared to the service while nothing can expand.
    expect(r.testField).toEqual([]);
    await user.click(button("Fix permissions"));
    await onStep(/Let Iamb see and type keys/);
  });

  it("shows the permissions step alone, with Done, when opened from Fix permissions", async () => {
    const r = rig({ mac: true, mode: "fix" });
    await renderOnboarding(r);
    expect(heading()).toMatch(/Let Iamb see and type keys/);
    expect(progress()).toBe("");
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Skip for now" })).toBeNull();
    expect((button("Done") as HTMLButtonElement).disabled).toBe(true);
    r.grantAll();
    const done = (await screen.findByRole("button", { name: /Done/ })) as HTMLButtonElement;
    await waitFor(() => expect(done.disabled).toBe(false));
    await userEvent.setup().click(done);
    expect(r.closed).toBe(1);
  });

  it("has no permissions step on Windows", async () => {
    await toSnippet({ permissions: NOT_REQUIRED });
    expect(progress()).toContain("Step 2 of 4");
  });
});

describe("Step 3: First snippet", () => {
  it("prefills the abbreviation, puts the keyboard in the text and shows the tip", async () => {
    await toSnippet();
    expect((screen.getByLabelText("When I type") as HTMLInputElement).value).toBe(";em");
    const types = screen.getByLabelText("Iamb types") as HTMLInputElement;
    expect(types.value).toBe("");
    expect(types.placeholder).toBe("your email address");
    expect(document.activeElement).toBe(types);
    expect(screen.getByText(/begin abbreviations with a character you rarely type before a word/)).toBeTruthy();
    expect(screen.getByText("Your email address is a good start. Change both sides to anything you like.")).toBeTruthy();
  });

  it("saves a plain snippet with the trigger a space, case exact and word start on, with Enter", async () => {
    const r = await toSnippet();
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Iamb types"), `${SAMPLE_EMAIL}{Enter}`);
    await onStep(/Try it here/);
    const saved = r.library.byAbbreviation(";em");
    expect(saved).toMatchObject({
      kind: "plain",
      body: SAMPLE_EMAIL,
      trigger: "delimiter",
      caseMode: "exact",
      wordStart: true,
      enabled: true,
      folderId: r.library.firstFolder.id,
    });
  });

  it("goes back to the abbreviation with Shift+Tab", async () => {
    await toSnippet();
    const user = userEvent.setup();
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(screen.getByLabelText("When I type"));
  });

  it("refuses an abbreviation with a space, empty text, and an abbreviation another snippet has", async () => {
    const r = await toSnippet();
    const user = userEvent.setup();
    const abbr = screen.getByLabelText("When I type");
    await user.clear(abbr);
    await user.type(abbr, ";e m");
    await user.type(screen.getByLabelText("Iamb types"), "text");
    await user.click(button(/Save and try it/));
    expect((await screen.findByRole("alert")).textContent).toBe("An abbreviation cannot contain spaces.");
    expect(abbr.getAttribute("aria-invalid")).toBe("true");

    await user.clear(abbr);
    await user.type(abbr, ";em");
    await user.clear(screen.getByLabelText("Iamb types"));
    await user.click(button(/Save and try it/));
    expect((await screen.findByRole("alert")).textContent).toBe("Type the text Iamb should type for you.");
    expect(r.library.calls).not.toContain("saveSnippet");

    // Another snippet holds the abbreviation (possible after an import).
    await user.type(screen.getByLabelText("Iamb types"), "text");
    await r.library.api.saveSnippet({
      folderId: r.library.firstFolder.id,
      abbreviation: ";em",
      label: "Taken",
      kind: "plain",
      body: "taken",
      trigger: "delimiter",
      caseMode: "exact",
      wordStart: true,
      enabled: true,
      tags: [],
    });
    await user.click(button(/Save and try it/));
    expect((await screen.findByRole("alert")).textContent).toBe(
      ";em is already used by another snippet. Pick a different abbreviation.",
    );
    await user.clear(abbr);
    await user.type(abbr, ";mail");
    await user.click(button(/Save and try it/));
    await onStep(/Try it here/);
  });

  it("skips the step when the library already has a snippet, and uses it as the example", async () => {
    const fake = createFake({ snippets: [{ abbreviation: ";addr", label: "Address", body: "1 Elm Street" }] });
    const r = rig({ library: fake });
    await renderOnboarding(r);
    await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
    await onStep(/Try it here/);
    expect(progress()).toContain("Step 2 of 3");
    expect(screen.getAllByText(";addr", { selector: ".ob-code" }).length).toBeGreaterThan(0);
    serviceTypes(";addr ");
    serviceTypes("1 Elm Street ");
    await onStep(/That is all there is to it/);
  });
});

describe("Step 4: Try it", () => {
  it("declares the test field on entering and withdraws it on leaving", async () => {
    const r = await toTry();
    expect(told(r)).toEqual([true]);
    expect(screen.getByText(/and then a/).textContent).toBe("Type ;em and then a space.");
    expect(screen.getByText(/Iamb expands when you finish the word with a space or with punctuation such as/)).toBeTruthy();
    expect(screen.getByText(/Enter and Tab do not expand a snippet\./)).toBeTruthy();
    expect(document.activeElement).toBe(field());
    expect(field().rows).toBe(3);
    await userEvent.setup().click(button("Back"));
    await onStep(/Make your first snippet/);
    expect(told(r)).toEqual([true, false]);
  });

  it("declares the field when it has the keyboard, not when the step mounts", async () => {
    const r = await toTry();
    // The field has the focus on entering, so it is declared; another window of Iamb that
    // held the keyboard at the time of the mount would have been declared before.
    expect(r.testField.length).toBeGreaterThan(0);
    const before = r.testField.length;
    fireEvent.blur(field());
    await new Promise((resolve) => setTimeout(resolve, 80));
    const idle = r.testField.length;
    // Without the keyboard it asks nothing (a blurred field is not renewed) ...
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(r.testField.length).toBe(idle);
    expect(idle).toBeGreaterThanOrEqual(before);
    // ... and a focus declares it again.
    fireEvent.focus(field());
    await waitFor(() => expect(r.testField.length).toBeGreaterThan(idle));
    expect(r.testField.at(-1)).toBe(true);
  });

  it("declares the field again while it has the keyboard, for a service that restarted", async () => {
    const r = await toTry();
    const first = r.testField.length;
    // The service forgot the field at a restart: nothing in the window changed, yet it is
    // declared again within a few polls.
    await waitFor(() => expect(r.testField.length).toBeGreaterThanOrEqual(first + 3));
    expect(r.testField.every(Boolean)).toBe(true);
  });

  it("withdraws the test field when the window closes", async () => {
    const r = rig();
    const { unmount } = await renderOnboarding(r);
    await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
    await userEvent.setup().type(await screen.findByLabelText("Iamb types"), SAMPLE_EMAIL);
    await userEvent.setup().click(button(/Save and try it/));
    await onStep(/Try it here/);
    unmount();
    await waitFor(() => expect(told(r)).toEqual([true, false]));
  });

  it("recognises the expansion, announces it and puts the keyboard on Continue", async () => {
    const r = await toTry();
    personTypes(";em ");
    serviceTypes(`${SAMPLE_EMAIL} `);
    await onStep(/That is all there is to it/);
    expect(screen.getByText("It works the same way in your mail, your browser and everywhere else you type.")).toBeTruthy();
    const status = screen.getAllByRole("status").find((s) => /replaced/.test(s.textContent ?? ""));
    expect(status?.textContent).toBe("Iamb replaced ;em with your snippet.");
    const next = button(/Continue/);
    await waitFor(() => expect(document.activeElement).toBe(next));
    // Try again empties the field and waits again; Continue goes on to the last step.
    await userEvent.setup().click(button("Try again"));
    await onStep(/Try it here/);
    expect(field().value).toBe("");
    personTypes(";em ");
    serviceTypes(`${SAMPLE_EMAIL} `);
    await userEvent.setup().click(await screen.findByRole("button", { name: /Continue/ }));
    await onStep(/Iamb is running/);
    expect(r.testField.at(-1)).toBe(false);
  });

  it("says Enter does not expand, and the button lets the snippet expand without a space", async () => {
    const r = await toTry();
    personTypes(";em");
    fireEvent.keyDown(field(), { key: "Enter" });
    const banner = await screen.findByText(/Enter does not expand a snippet\./);
    expect(banner.closest(".ob-banner")?.textContent).toContain("Apps act on Enter at once (they send the message or move on) before Iamb could type.");
    await userEvent.setup().click(button("Expand without a space"));
    await waitFor(() => expect(r.library.byAbbreviation(";em")?.trigger).toBe("immediate"));
    expect(r.library.byAbbreviation(";em")?.wordStart).toBe(false);
    expect(field().value).toBe("");
    expect(screen.queryByText(/Enter does not expand/)).toBeNull();
    expect(screen.getByText(/^Type/).textContent).toBe("Type ;em.");
    expect((screen.getByRole("switch") as HTMLInputElement).checked).toBe(true);
  });

  it("keeps Tab for itself once to say the same, then lets it move focus", async () => {
    await toTry();
    personTypes(";em");
    const first = fireEvent.keyDown(field(), { key: "Tab" });
    expect(first).toBe(false);
    expect(await screen.findByText(/Tab does not expand a snippet\./)).toBeTruthy();
    const second = fireEvent.keyDown(field(), { key: "Tab" });
    expect(second).toBe(true);
  });

  it("flips the snippet to expand the moment it is finished, and back", async () => {
    const r = await toTry();
    const user = userEvent.setup();
    const sw = screen.getByRole("switch", { name: /Expand the moment I finish typing ;em, without a space/ }) as HTMLInputElement;
    expect(sw.checked).toBe(false);
    await user.tab();
    expect(document.activeElement).toBe(sw);
    await user.keyboard(" ");
    await waitFor(() => expect(r.library.byAbbreviation(";em")).toMatchObject({ trigger: "immediate", wordStart: false }));
    await user.click(sw);
    await waitFor(() => expect(r.library.byAbbreviation(";em")).toMatchObject({ trigger: "delimiter", wordStart: true }));
  });

  describe("when nothing happens", () => {
    async function typedWithNothing(options: Parameters<typeof rig>[0] = {}) {
      const r = await toTry(options);
      personTypes(";em ");
      return r;
    }

    it("blames the keyboard first, in the words of the system", async () => {
      const mac = await typedWithNothing({ mac: true, permissions: { accessibility: "granted", inputMonitoring: "granted" }, service: { listener: false } });
      expect(await screen.findByText("Iamb is not seeing the keyboard.")).toBeTruthy();
      expect(screen.getByText(/macOS sometimes needs Iamb’s helper to restart after a permission changes\./)).toBeTruthy();
      const restart = screen.getByRole("button", { name: "Restart the helper" });
      await waitFor(() => expect(document.activeElement).toBe(restart));
      await userEvent.setup().click(restart);
      expect(mac.restarts).toBe(1);
      // The service takes the field again after the restart.
      await waitFor(() => expect(mac.testField.filter(Boolean).length).toBeGreaterThanOrEqual(2));
    });

    it("says the background service is not running on Windows", async () => {
      const r = await typedWithNothing({ service: { listener: false } });
      expect(await screen.findByText(/Its background service is not running\./)).toBeTruthy();
      await userEvent.setup().click(screen.getByRole("button", { name: "Start it again" }));
      expect(r.restarts).toBe(1);
    });

    it("says what to do when the helper cannot be restarted from here", async () => {
      const r = await typedWithNothing({ service: { listener: false } });
      r.failRestart();
      await userEvent.setup().click(await screen.findByRole("button", { name: "Start it again" }));
      expect(await screen.findByText(/Quit Iamb from its menu and open it again\./)).toBeTruthy();
    });

    it("then says Iamb is paused and resumes it", async () => {
      const r = await typedWithNothing({ service: { paused: true } });
      expect(await screen.findByText("Iamb is paused.")).toBeTruthy();
      await userEvent.setup().click(screen.getByRole("button", { name: "Resume" }));
      expect(r.resumed).toBe(1);
      await waitFor(() => expect(screen.queryByText("Iamb is paused.")).toBeNull());
    });

    it("then says a permission was turned off and goes to the permissions step", async () => {
      const r = rig({ mac: true, permissions: { accessibility: "granted", inputMonitoring: "granted" } });
      await renderOnboarding(r);
      const user = userEvent.setup();
      await user.click(await screen.findByRole("button", { name: /Get started/ }));
      await user.type(await screen.findByLabelText("Iamb types"), SAMPLE_EMAIL);
      await user.click(button(/Save and try it/));
      await onStep(/Try it here/);
      r.setPermissions({ accessibility: "denied", inputMonitoring: "granted" });
      personTypes(";em ");
      expect(await screen.findByText("A permission was turned off.")).toBeTruthy();
      await user.click(screen.getByRole("button", { name: "Fix" }));
      await onStep(/Let Iamb see and type keys/);
    });

    it("then says the keyboard is composing text when an input method is on, with no button", async () => {
      await toTry();
      fireEvent.compositionStart(field());
      personTypes(";em ");
      expect(await screen.findByText("Your keyboard is composing text.")).toBeTruthy();
      expect(screen.getByText(/An input method is on\. Switch to a plain layout to try\./)).toBeTruthy();
      expect(document.querySelector(".ob-banner.bad button")).toBeNull();
    });

    it("finally points to Why didn’t it expand?", async () => {
      const r = await typedWithNothing();
      expect(await screen.findByText("Something else is in the way.")).toBeTruthy();
      await userEvent.setup().click(screen.getByRole("button", { name: "Open “Why didn’t it expand?”" }));
      expect(r.whyOpened).toBe(1);
    });

    it("is asked for at once by It does not work, and Skip this step goes to the end", async () => {
      await toTry();
      const user = userEvent.setup();
      await user.click(button("It does not work"));
      expect(await screen.findByText("Something else is in the way.")).toBeTruthy();
      await user.click(button("Skip this step"));
      await onStep(/Iamb is running/);
    });

    it("clears the banner when the person keeps typing", async () => {
      await typedWithNothing();
      await screen.findByText("Something else is in the way.");
      personTypes(";em x");
      expect(screen.queryByText("Something else is in the way.")).toBeNull();
    });

    it("does not ask while the person is still typing the abbreviation", async () => {
      const r = await toTry();
      personTypes(";e");
      await new Promise((resolve) => setTimeout(resolve, 120));
      expect(screen.queryByRole("alert")).toBeNull();
      expect(r.calls).not.toContain("openWhy");
    });

    it("explains it at once when the system cannot take the field", async () => {
      const r = rig();
      r.failDeclare();
      await renderOnboarding(r);
      const user = userEvent.setup();
      await user.click(await screen.findByRole("button", { name: /Get started/ }));
      await user.type(await screen.findByLabelText("Iamb types"), SAMPLE_EMAIL);
      await user.click(button(/Save and try it/));
      expect(await screen.findByText("Something else is in the way.")).toBeTruthy();
    });
  });
});

describe("Step 5: Done", () => {
  async function toDone(options: Parameters<typeof rig>[0] = {}) {
    const r = await toTry(options);
    await userEvent.setup().click(button("It does not work"));
    await userEvent.setup().click(await screen.findByRole("button", { name: "Skip this step" }));
    await onStep(/Iamb is running/);
    return r;
  }

  it("finishes onboarding, sets the login item and says where Iamb lives (Windows)", async () => {
    const r = await toDone();
    await waitFor(() => expect(r.finished).toBe(1));
    expect(r.logins).toEqual([true]);
    expect(r.testField.at(-1)).toBe(false);
    expect(screen.getByText(/It lives in the notification area, at the bottom right of your screen\. Close this window whenever you like: it keeps working\./)).toBeTruthy();
    expect(screen.getByText("Pause Iamb from its tray icon.")).toBeTruthy();
    expect(screen.getByText(/Forgot an abbreviation\? Press/)).toBeTruthy();
    expect(screen.getByRole("img", { name: /Control, Shift, Space|Ctrl/ })).toBeTruthy();
    expect((screen.getByRole("checkbox", { name: "Start Iamb when I log in" }) as HTMLInputElement).checked).toBe(true);
    const open = button(/Open my library/);
    await waitFor(() => expect(document.activeElement).toBe(open));
    expect(progress()).toContain("Step 4 of 4");
  });

  it("says menu bar and writes the shortcut with symbols on macOS", async () => {
    const r = rig({ mac: true, permissions: { accessibility: "granted", inputMonitoring: "granted" } });
    await renderOnboarding(r);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /Get started/ }));
    await user.type(await screen.findByLabelText("Iamb types"), SAMPLE_EMAIL);
    await user.click(button(/Save and try it/));
    await user.click(await screen.findByRole("button", { name: "It does not work" }));
    await user.click(await screen.findByRole("button", { name: "Skip this step" }));
    await onStep(/Iamb is running/);
    expect(screen.getByText(/It lives in the menu bar, at the top right of your screen\./)).toBeTruthy();
    expect(screen.getByText("Pause Iamb from its menu-bar icon.")).toBeTruthy();
    expect(document.querySelector("b [role='img']")?.textContent).toBe("⌃⇧Space");
  });

  it("opens the library with the new snippet selected", async () => {
    const r = await toDone();
    await userEvent.setup().keyboard("{Enter}");
    await waitFor(() => expect(r.librariesOpened).toEqual([true]));
    expect(r.selections).toEqual([r.library.byAbbreviation(";em")?.id]);
  });

  it("closes with the button and with Esc, leaving Iamb running", async () => {
    const r = await toDone();
    const user = userEvent.setup();
    await user.click(button("Close"));
    expect(r.closed).toBe(1);
    await user.keyboard("{Escape}");
    expect(r.closed).toBe(2);
    expect(r.librariesOpened).toEqual([]);
  });

  it("turns the login item off and on with the checkbox, and says when the system refuses", async () => {
    const r = await toDone();
    const user = userEvent.setup();
    const box = screen.getByRole("checkbox", { name: "Start Iamb when I log in" }) as HTMLInputElement;
    await user.click(box);
    await waitFor(() => expect(r.logins).toEqual([true, false]));
    expect(box.checked).toBe(false);
    r.failLogin();
    await user.click(box);
    expect(await screen.findByText(/Iamb could not change this\./)).toBeTruthy();
  });

  it("asks for another shortcut when the system refused the default one", async () => {
    const r = await toDone({ hotkeyRefused: true });
    expect(await screen.findByText("Choose a shortcut for search")).toBeTruthy();
    const recorder = screen.getByRole("button", { name: "Choose a shortcut for search" });
    recorder.focus();
    r.refuseChord("Ctrl+Alt+K");
    fireEvent.keyDown(recorder, { key: "k", code: "KeyK", ctrlKey: true, altKey: true });
    expect(await screen.findByText("Another app already uses this shortcut.")).toBeTruthy();
    fireEvent.keyDown(recorder, { key: "j", code: "KeyJ", ctrlKey: true, altKey: true });
    await waitFor(() => expect(r.settings).toEqual([{ key: "palette_hotkey", value: "Ctrl+Alt+J" }]));
    expect(await screen.findByText(/Forgot an abbreviation\? Press/)).toBeTruthy();
  });
});

describe("what is typed", () => {
  it("never reaches a command: no call carries the test field's text", async () => {
    const r = await toTry();
    personTypes(";em ");
    serviceTypes(`${SAMPLE_EMAIL} `);
    await onStep(/That is all there is to it/);
    const seen = JSON.stringify(r.calls);
    expect(seen).not.toContain(SAMPLE_EMAIL.slice(0, 5));
    expect(seen).not.toContain(";em ");
  });

  it("tells the web view not to remember or correct it", async () => {
    await toTry();
    const el = field();
    expect(el.getAttribute("autocomplete")).toBe("off");
    expect(el.getAttribute("autocorrect")).toBe("off");
    expect(el.getAttribute("autocapitalize")).toBe("off");
    expect(el.getAttribute("spellcheck")).toBe("false");
  });

  it("does not need the service to be named in the unrelated NOTHING_GRANTED fixture", () => {
    expect(NOTHING_GRANTED.accessibility).toBe("denied");
  });
});
