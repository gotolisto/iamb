// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { SettingsView } from "./SettingsView";
import { rig } from "./testkit";

afterEach(cleanup);

const loginSwitch = () => screen.getByRole("switch", { name: "Start Iamb when I log in" });

async function open(r: ReturnType<typeof rig>) {
  render(<SettingsView env={r.env} />);
  await screen.findByRole("heading", { name: "General" });
  await waitFor(() => expect((loginSwitch() as HTMLButtonElement).disabled).toBe(false));
}

describe("Start Iamb when I log in (Settings → General)", () => {
  it("shows what the system has and changes it at once, from the keyboard", async () => {
    const r = rig();
    await open(r);
    expect(loginSwitch().getAttribute("aria-checked")).toBe("true");
    loginSwitch().focus();
    await userEvent.setup().keyboard(" ");
    await waitFor(() => expect(loginSwitch().getAttribute("aria-checked")).toBe("false"));
    expect(r.settings.login()).toBe("disabled");
    await userEvent.setup().click(loginSwitch());
    await waitFor(() => expect(loginSwitch().getAttribute("aria-checked")).toBe("true"));
    expect(r.settings.calls).toContain("loginItem true");
  });

  it("explains an approval macOS still needs", async () => {
    await open(rig({}, { mac: true }, { login: "requires_approval" }));
    expect(loginSwitch().getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText("macOS asks you to approve this once, in System Settings, under Login Items.")).toBeTruthy();
  });

  it("is off and cannot be changed where this copy of Iamb has no login item", async () => {
    const r = rig({}, {}, { login: "unavailable" });
    render(<SettingsView env={r.env} />);
    await screen.findByText("This copy of Iamb cannot start at login. Open it from the Start menu.");
    expect((loginSwitch() as HTMLButtonElement).disabled).toBe(true);
    expect(loginSwitch().getAttribute("aria-describedby")).toBeTruthy();
  });

  it("says so when the system refuses the change", async () => {
    const r = rig();
    await open(r);
    r.settings.refuseLoginChanges();
    await userEvent.setup().click(loginSwitch());
    expect(
      (await screen.findByRole("alert")).textContent,
    ).toBe("Iamb could not change this. It is not a problem: you can start it from the Start menu.");
  });
});
