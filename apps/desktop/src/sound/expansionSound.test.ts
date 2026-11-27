import { describe, expect, it, vi } from "vitest";

import type { UiEvent } from "../ipc";
import { chimePlayer, SOUND_KEY, soundEnabled, startExpansionSound, type SoundDeps } from "./expansionSound";

/** Fake dependencies: `emit` plays an event to the handler, `settings` is what the service says. */
function fakes(settings: () => Promise<Record<string, string>>) {
  let handler: (e: UiEvent) => void = () => {};
  const unlisten = vi.fn();
  const play = vi.fn();
  const deps: SoundDeps = {
    listen: async (h) => {
      handler = h;
      return unlisten;
    },
    getSettings: settings,
    play,
  };
  return { deps, play, unlisten, emit: (e: UiEvent) => handler(e) };
}

const finished: UiEvent = { type: "expansion_finished" };
const tick = () => new Promise((r) => setTimeout(r, 0));

describe("soundEnabled", () => {
  it("is on unless the setting says off", () => {
    expect(soundEnabled(undefined)).toBe(true);
    expect(soundEnabled({})).toBe(true);
    expect(soundEnabled({ [SOUND_KEY]: "on" })).toBe(true);
    expect(soundEnabled({ [SOUND_KEY]: "off" })).toBe(false);
  });
});

describe("startExpansionSound", () => {
  it("plays once for each finished expansion, by default", async () => {
    const f = fakes(async () => ({}));
    await startExpansionSound(f.deps);
    f.emit(finished);
    f.emit(finished);
    expect(f.play).toHaveBeenCalledTimes(2);
  });

  it("stays silent for other events", async () => {
    const f = fakes(async () => ({}));
    await startExpansionSound(f.deps);
    f.emit({ type: "palette_requested" });
    f.emit({ type: "diagnostic_added", code: "x", snippet: null, app: null, at: 1 });
    expect(f.play).not.toHaveBeenCalled();
  });

  it("stays silent when the setting is off, and follows a change of it", async () => {
    let values: Record<string, string> = { [SOUND_KEY]: "off" };
    const f = fakes(async () => values);
    await startExpansionSound(f.deps);
    f.emit(finished);
    expect(f.play).not.toHaveBeenCalled();

    values = {};
    f.emit({ type: "settings_changed" });
    await tick();
    f.emit(finished);
    expect(f.play).toHaveBeenCalledTimes(1);

    values = { [SOUND_KEY]: "off" };
    f.emit({ type: "settings_changed" });
    await tick();
    f.emit(finished);
    expect(f.play).toHaveBeenCalledTimes(1);
  });

  it("is on while the settings cannot be read, and reads them at the next event", async () => {
    let up = false;
    const f = fakes(async () => {
      if (!up) throw new Error("service not there");
      return { [SOUND_KEY]: "off" };
    });
    await startExpansionSound(f.deps);
    up = true;
    // The first read failed; this event reads again and finds the sound off.
    f.emit(finished);
    await tick();
    expect(f.play).not.toHaveBeenCalled();
  });

  it("plays when the settings still cannot be read (the default is on)", async () => {
    const f = fakes(async () => {
      throw new Error("service not there");
    });
    await startExpansionSound(f.deps);
    f.emit(finished);
    await tick();
    expect(f.play).toHaveBeenCalledTimes(1);
  });

  it("hands back the way to stop listening", async () => {
    const f = fakes(async () => ({}));
    const stop = await startExpansionSound(f.deps);
    stop();
    expect(f.unlisten).toHaveBeenCalledTimes(1);
  });
});

describe("chimePlayer", () => {
  const element = () => {
    const el = { currentTime: 5, play: vi.fn(() => Promise.resolve()) };
    return { el, create: vi.fn(() => el as unknown as HTMLAudioElement) };
  };

  it("makes one element and rewinds it for each play", () => {
    const { el, create } = element();
    const play = chimePlayer("chime.mp3", create);
    play();
    play();
    expect(create).toHaveBeenCalledTimes(1);
    expect(create).toHaveBeenCalledWith("chime.mp3");
    expect(el.play).toHaveBeenCalledTimes(2);
    expect(el.currentTime).toBe(0);
  });

  it("never throws when the audio cannot play", async () => {
    const { el, create } = element();
    el.play.mockReturnValue(Promise.reject(new Error("not allowed")));
    expect(() => chimePlayer("chime.mp3", create)()).not.toThrow();
    await tick();
    expect(() =>
      chimePlayer("chime.mp3", () => {
        throw new Error("no audio output");
      })(),
    ).not.toThrow();
  });
});
