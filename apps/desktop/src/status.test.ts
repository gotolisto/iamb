import { describe, expect, it } from "vitest";

import type { ServiceView } from "./ipc";
import { statusText } from "./status";

const service: ServiceView = {
  version: "0.0.0",
  paused: false,
  pausedKept: false,
  suspended: false,
  snippets: 3,
  revision: 1,
  listener: true,
  memoryLocked: true,
};

describe("statusText", () => {
  it("follows the tray menu's wording", () => {
    expect(statusText({ connection: "connected", service })).toBe("Service: running");
    expect(statusText({ connection: "connected", service: { ...service, paused: true } })).toBe(
      "Service: paused",
    );
    expect(
      statusText({ connection: "connected", service: { ...service, listener: false } }),
    ).toBe("Service: not listening");
    expect(statusText({ connection: "connecting", service: null })).toBe("Service: connecting…");
    expect(statusText({ connection: "disconnected", service: null })).toBe(
      "Service: not running, retrying",
    );
  });
});
