// @vitest-environment jsdom
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createFake, renderLibrary } from "./testkit";

afterEach(cleanup);

const banner = () => screen.getByRole("alert");

describe("the library's \"Iamb is not running\" banner", () => {
  it("offers Start it again, reachable with the keyboard, and waits while the service starts", async () => {
    const restartService = vi.fn(() => Promise.resolve());
    await renderLibrary(createFake(), { connection: "disconnected", restartService });
    const button = within(banner()).getByRole("button", { name: "Start it again" });
    button.focus();
    await userEvent.setup().keyboard("{Enter}");
    expect(restartService).toHaveBeenCalledTimes(1);
    const waiting = within(banner()).getByRole("button", { name: "Starting…" }) as HTMLButtonElement;
    expect(waiting.disabled).toBe(true);
    expect(waiting.getAttribute("aria-busy")).toBe("true");
  });

  it("says when it could not start the service, and the button can be pressed again", async () => {
    const restartService = vi.fn(() => Promise.reject(new Error("unavailable")));
    await renderLibrary(createFake(), { connection: "disconnected", restartService });
    await userEvent.setup().click(within(banner()).getByRole("button", { name: "Start it again" }));
    expect(
      await within(banner()).findByText(/Iamb could not start it from here\. Quit Iamb from its menu and open it again\./),
    ).toBeTruthy();
    expect((within(banner()).getByRole("button", { name: "Start it again" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("has no button where the app cannot restart the service", async () => {
    await renderLibrary(createFake(), { connection: "disconnected" });
    expect(within(banner()).queryByRole("button")).toBeNull();
  });

  it("is gone once the service answers", async () => {
    const restartService = vi.fn(() => Promise.resolve());
    const { rerender, env } = await renderLibrary(createFake(), { connection: "disconnected", restartService });
    await userEvent.setup().click(within(banner()).getByRole("button", { name: "Start it again" }));
    const { Library } = await import("./Library");
    rerender(<Library env={{ ...env, connection: "connected" }} />);
    expect(screen.queryByText(/Its background service stopped/)).toBeNull();
  });
});
