// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { isAbout } from "./api";
import { SettingsView } from "./SettingsView";
import { rig } from "./testkit";

afterEach(cleanup);

async function openAbout(r: ReturnType<typeof rig>) {
  const user = userEvent.setup();
  render(<SettingsView env={r.env} />);
  await screen.findByRole("heading", { name: "General" });
  await user.click(screen.getByRole("button", { name: "About" }));
  return user;
}

describe("Settings → About", () => {
  it("shows the version, the licence, the source and the log folder", async () => {
    await openAbout(rig());
    expect(await screen.findByText("Iamb 0.1.0")).toBeTruthy();
    expect(screen.getByText("AGPL-3.0-or-later")).toBeTruthy();
    expect(screen.getByText("/Users/test/Library/Application Support/Iamb/logs")).toBeTruthy();
  });

  it("opens the log folder from the keyboard", async () => {
    const r = rig();
    const user = await openAbout(r);
    const button = await screen.findByRole("button", { name: "Open the log folder" });
    button.focus();
    await user.keyboard("{Enter}");
    expect(r.settings.calls).toContain("openLogFolder");
  });

  it("says where the folder is when it cannot be opened", async () => {
    const r = rig();
    r.settings.refuseOpenFolder();
    const user = await openAbout(r);
    await user.click(await screen.findByRole("button", { name: "Open the log folder" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Iamb could not open the folder. It is /Users/test/Library/Application Support/Iamb/logs.",
    );
  });

  it("checks the shape of the answer", () => {
    expect(isAbout({ version: "0.1.0", logFolder: "/x" })).toBe(true);
    expect(isAbout({ version: "0.1.0" })).toBe(false);
    expect(isAbout(null)).toBe(false);
  });
});
