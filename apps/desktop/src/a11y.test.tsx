// @vitest-environment jsdom
// Automated accessibility checks (task 3.11): axe-core over every view and the states that
// matter, in jsdom. Zero violations of any impact is the bar here; `expectAccessible` lists
// what it finds. What jsdom cannot judge (contrast, target size, scrolling regions) is checked
// in Chromium by `scripts/keyboard-run.mjs`. See docs/test-strategy.md §8.9.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";

import { labelInName } from "../scripts/label-in-name.mjs";
import { expectAccessible, tally } from "./a11y";
import type { FormEvent, FormOpen } from "./form/api";
import { FormView } from "./form/FormView";
import type { FieldView } from "./ipc";
import { Diagnostics } from "./library/diagnostics";
import { SettingsView } from "./library/settings/SettingsView";
import { SECTIONS } from "./library/settings/section";
import { diagnostic, rig as settingsRig } from "./library/settings/testkit";
import { createFake, renderLibrary, setContent, typeContent } from "./library/testkit";
import {
  NOTHING_GRANTED,
  NOT_REQUIRED,
  SAMPLE_EMAIL,
  onStep,
  personTypes,
  renderOnboarding,
  rig as onboardingRig,
  serviceTypes,
} from "./onboarding/testkit";
import type { PaletteApi, PaletteEvent, PaletteResults, ListenPalette } from "./palette/api";
import { PaletteView } from "./palette/PaletteView";
import { hit, openOf } from "./palette/testkit";

afterEach(cleanup);

afterAll(() => {
  // The counts the report quotes: `pnpm vitest run src/a11y.test.tsx` with VITE_A11Y_TALLY=1.
  if (import.meta.env.VITE_A11Y_TALLY) console.log(`A11Y_TALLY ${JSON.stringify(tally)}`);
});

const library = () =>
  createFake({
    folders: [{ name: "Replies", defaults: { abbreviationPrefix: ";r", trigger: null, caseMode: null } }],
    tags: [
      { name: "urgent", color: "red" },
      { name: "español", color: "blue" },
    ],
    snippets: [
      { abbreviation: ";rty", label: "Thanks for your order", body: "Thank you for your order." },
      { abbreviation: ";rlate", label: "Late delivery", body: 'Hi {field name="First name"}, sorry.' },
      { abbreviation: ";rhours", label: "Opening hours", body: "We are open Tuesday to Saturday.", tags: [] },
      { abbreviation: ";rsig", label: "Signature", kind: "rich", body: "**Kim Lee**\n\n- one\n- two" },
    ],
  });

async function openLibrary() {
  const user = userEvent.setup();
  const ctx = await renderLibrary(library());
  await screen.findAllByRole("option");
  return { user, ...ctx };
}

describe("library window", () => {
  it("list", async () => {
    await openLibrary();
    await expectAccessible("library: list");
  });

  it("list with a filter that finds nothing", async () => {
    const { user } = await openLibrary();
    await user.keyboard("{Control>}f{/Control}");
    await user.type(screen.getByRole("searchbox"), "zzz");
    await expectAccessible("library: list, filter without a match");
  });

  it("empty folder and empty library", async () => {
    await renderLibrary(createFake({ folders: [{ name: "Replies" }] }));
    await expectAccessible("library: empty");
  });

  it("service not running", async () => {
    await renderLibrary(library(), { connection: "disconnected" });
    await expectAccessible("library: service not running");
  });

  it("editor, plain text, saved snippet", async () => {
    const { user } = await openLibrary();
    await user.click(screen.getByRole("option", { name: /^Late delivery/ }));
    await screen.findByLabelText("Content");
    await expectAccessible("library: editor, plain");
  });

  it("editor, new draft with errors and the options open", async () => {
    const { user } = await openLibrary();
    await user.keyboard("{Control>}n{/Control}");
    const abbreviation = await screen.findByLabelText("Abbreviation");
    await user.type(abbreviation, ";rty");
    await typeContent("oops {");
    await user.click(screen.getByRole("button", { name: "Options" }));
    await expectAccessible("library: editor, new, with messages and options");
  });

  it("editor, formatted", async () => {
    const { user } = await openLibrary();
    await user.click(screen.getByRole("option", { name: /^Signature/ }));
    await screen.findByRole("toolbar", { name: "Formatting" });
    await expectAccessible("library: editor, formatted");
  });

  it("editor, preview", async () => {
    const { user } = await openLibrary();
    await user.click(screen.getByRole("option", { name: /^Late delivery/ }));
    await screen.findByLabelText("Content");
    await user.keyboard("{Control>}{Shift>}p{/Shift}{/Control}");
    await expectAccessible("library: editor, preview");
  });

  it("the unsaved-changes dialog", async () => {
    const { user } = await openLibrary();
    await user.click(screen.getByRole("option", { name: /^Opening hours/ }));
    await screen.findByLabelText("Content");
    await setContent("changed");
    await user.click(screen.getByRole("option", { name: /^Late delivery/ }));
    await screen.findByRole("alertdialog");
    await expectAccessible("library: dialog, unsaved changes");
  });

  it("the new folder dialog and the new tag dialog", async () => {
    const { user } = await openLibrary();
    await user.keyboard("{Control>}{Shift>}n{/Shift}{/Control}");
    await screen.findByRole("dialog");
    await expectAccessible("library: dialog, new folder");
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "New tag" }));
    await screen.findByRole("dialog", { name: "New tag" });
    await expectAccessible("library: dialog, new tag");
  });

  it("a folder's menu", async () => {
    const { user } = await openLibrary();
    await user.click(await screen.findByRole("treeitem", { name: /^Replies/ }));
    await user.click(screen.getByRole("button", { name: "More actions for Replies" }));
    await screen.findByRole("menu");
    await expectAccessible("library: menu, folder actions");
  });

  it("the toast after a delete", async () => {
    const { user } = await openLibrary();
    await user.click(screen.getByRole("option", { name: /^Opening hours/ }));
    await user.keyboard("{Delete}");
    await screen.findByText("Deleted “Opening hours”.");
    await expectAccessible("library: toast with Undo");
  });

  it("library health and the diagnostics view inside the window", async () => {
    const fake = library();
    await fake.api.saveSnippet({
      folderId: fake.firstFolder.id, abbreviation: ";bad", label: "Bad", kind: "plain", body: "an {unclosed",
      trigger: "delimiter", caseMode: "insensitive", wordStart: true, enabled: true, tags: [],
    });
    const user = userEvent.setup();
    await renderLibrary(fake);
    await screen.findAllByRole("option");
    await user.click(screen.getByRole("button", { name: /Library health/ }));
    await screen.findByRole("region", { name: "Will not expand: error in the snippet" });
    await expectAccessible("health: with a broken snippet");
  });
});

describe("library health", () => {
  it("with nothing to fix", async () => {
    const user = userEvent.setup();
    await renderLibrary(library());
    await screen.findAllByRole("option");
    await user.click(screen.getByRole("button", { name: /Library health/ }));
    await screen.findByText(/Nothing to fix/);
    await expectAccessible("health: nothing to fix");
  });
});

describe("settings", () => {
  for (const section of SECTIONS) {
    it(`the ${section.label} section`, async () => {
      const r = settingsRig({}, {}, {
        exclusions: [{ appId: "mail.exe", displayName: "Mail", createdAt: 1 }],
        diagnostics: [diagnostic({ code: "suspended_secure_field" })],
      });
      render(<SettingsView env={r.env} />);
      await screen.findByRole("heading", { name: "General" });
      await userEvent.setup().click(screen.getByRole("button", { name: section.label }));
      await screen.findByRole("heading", { name: section.label });
      await expectAccessible(`settings: ${section.label}`);
    });
  }

  it("the service is not running", async () => {
    const r = settingsRig();
    r.settings.setDown("disconnected");
    render(<SettingsView env={r.env} />);
    await screen.findByRole("heading", { name: "General" });
    await expectAccessible("settings: service not running");
  });
});

describe("why didn’t it expand", () => {
  it("empty and with entries", async () => {
    const r = settingsRig({}, {}, {
      diagnostics: [
        diagnostic({ code: "suspended_secure_field", at: Date.UTC(2026, 10, 20, 9, 5) }),
        diagnostic({ code: "no_last_form", at: Date.UTC(2026, 10, 20, 9, 10) }),
      ],
    });
    render(<Diagnostics env={r.env} onOpenSnippet={() => {}} onOpenSettings={() => {}} />);
    await screen.findByRole("list", { name: "Entries, newest first" });
    await expectAccessible("diagnostics: entries");
    cleanup();
    render(<Diagnostics env={settingsRig().env} onOpenSnippet={() => {}} onOpenSettings={() => {}} />);
    await screen.findByText("Nothing has been held back since Iamb started.");
    await expectAccessible("diagnostics: empty");
  });
});

// ---------------------------------------------------------------- the fill-in form

const field = (name: string, kind: FieldView["kind"], options: string[] = [], d = ""): FieldView => ({
  name,
  kind,
  options,
  default: d,
});

function formHarness(form: FormOpen) {
  let handler: ((e: FormEvent) => void) | undefined;
  const api = {
    ready: vi.fn(() => Promise.resolve()),
    current: vi.fn(() => Promise.resolve(form)),
    laidOut: vi.fn(() => Promise.resolve()),
    painted: vi.fn(() => Promise.resolve()),
    submit: vi.fn((_s: number, _v: Record<string, string>) => Promise.resolve()),
    cancel: vi.fn(() => Promise.resolve()),
  };
  render(
    <FormView
      api={api}
      listen={(h) => {
        handler = h;
        return Promise.resolve(() => {});
      }}
      os="win"
      onNextPaint={(report) => report(1)}
    />,
  );
  return {
    api,
    send: async (e: FormEvent) => {
      await act(async () => {
        handler?.(e);
        await Promise.resolve();
      });
    },
  };
}

const FORM: FormOpen = {
  session: 7,
  snippet: "01010101-0101-0101-0101-010101010101",
  label: "Repair quote",
  abbreviation: ";rquote",
  app: "Mail",
  fields: [
    field("Customer", "text", [], "you"),
    field("Bike", "choice", ["Road", "Gravel", "City", "Cargo"], "Gravel"),
    field("Rim", "choice", ["26", "27.5", "29", "650b", "700c", "700c, 32 holes", "Other"], "700c"),
    field("Work needed", "area"),
    field("Parts in stock", "toggle", [], "no"),
  ],
  lastValues: null,
};

describe("the fill-in form", () => {
  it("every kind of field", async () => {
    const h = formHarness(FORM);
    await screen.findByRole("dialog", { name: "Repair quote" });
    await h.send({ type: "shown", session: 7 });
    await expectAccessible("form: every field type");
  });

  it("with remembered answers", async () => {
    const h = formHarness({ ...FORM, lastValues: { Customer: "Jun", Bike: "Road" } as never });
    await screen.findByRole("dialog", { name: "Repair quote" });
    await h.send({ type: "shown", session: 7 });
    await expectAccessible("form: remembered answers");
  });

  it("could not get back to the app", async () => {
    const h = formHarness(FORM);
    await screen.findByRole("dialog", { name: "Repair quote" });
    await h.send({ type: "shown", session: 7 });
    h.api.submit.mockImplementationOnce(() => Promise.reject({ code: "focus_lost", message: "gone" }));
    fireEvent.click(screen.getByRole("button", { name: /Insert/ }));
    await screen.findByRole("alert");
    await expectAccessible("form: could not get back banner");
  });
});

// ---------------------------------------------------------------- the search palette

function paletteHarness(open = openOf(), recent: PaletteResults = { total: 76, hits: [] }) {
  let handler: ((e: PaletteEvent) => void) | undefined;
  const answers = new Map<string, PaletteResults>();
  const api = {
    ready: vi.fn(() => Promise.resolve()),
    current: vi.fn(() => Promise.resolve(open)),
    painted: vi.fn(() => Promise.resolve()),
    search: vi.fn((q: string) => Promise.resolve(answers.get(q) ?? { total: 76, hits: [] })),
    recent: vi.fn(() => Promise.resolve(recent)),
    preview: vi.fn((_id: string) => Promise.resolve(null)),
    pick: vi.fn((_s: number, _a: unknown) => Promise.resolve()),
  } satisfies PaletteApi;
  const listen: ListenPalette = (h) => {
    handler = h;
    return Promise.resolve(() => {});
  };
  render(<PaletteView api={api} listen={listen} os="win" onNextPaint={(report) => report(1)} />);
  return {
    api,
    answers,
    shown: async () => {
      await act(async () => {
        handler?.({ type: "shown", session: open.session });
        await Promise.resolve();
      });
    },
  };
}

describe("the search palette", () => {
  it("the first state", async () => {
    const h = paletteHarness();
    await screen.findByRole("dialog", { name: "Search snippets" });
    await h.shown();
    await expectAccessible("palette: nothing typed");
  });

  it("recent snippets", async () => {
    const h = paletteHarness(openOf({ lastForm: { label: "Repair quote" } as never }), {
      total: 76,
      hits: [hit({ label: "Newest", abbreviation: ";n" }, { field: null }), hit({ label: "Older", abbreviation: ";o" }, { field: null })],
    });
    await screen.findAllByRole("option");
    await h.shown();
    await expectAccessible("palette: recent snippets");
  });

  it("results", async () => {
    const h = paletteHarness();
    h.answers.set("ret", {
      total: 76,
      hits: [
        hit({ label: "Return label", abbreviation: ";rret" }, { field: "label", ranges: [[0, 3]] as never }),
        hit({ label: "Retail hours", abbreviation: ";hrs", kind: "form" as never }, { field: "label", ranges: [[0, 3]] as never }),
        hit({ label: "Off", abbreviation: ";off", enabled: false }, { field: "abbreviation", ranges: [] }),
      ],
    });
    await screen.findByRole("dialog", { name: "Search snippets" });
    await h.shown();
    await userEvent.setup().type(screen.getByRole("combobox"), "ret");
    await screen.findAllByRole("option");
    await expectAccessible("palette: results");
  });

  it("no match", async () => {
    const h = paletteHarness();
    await screen.findByRole("dialog", { name: "Search snippets" });
    await h.shown();
    await userEvent.setup().type(screen.getByRole("combobox"), "warranty");
    await screen.findByText("No snippet matches “warranty”.");
    await expectAccessible("palette: no match");
  });
});

// ---------------------------------------------------------------- onboarding

describe("onboarding", () => {
  it("step 1: welcome", async () => {
    await renderOnboarding(onboardingRig());
    await screen.findByRole("button", { name: /Get started/ });
    await expectAccessible("onboarding: welcome");
  });

  it("step 2: permissions (macOS)", async () => {
    await renderOnboarding(onboardingRig({ mac: true, permissions: NOTHING_GRANTED }));
    await userEvent.setup().click(await screen.findByRole("button", { name: /Get started/ }));
    await onStep(/Let Iamb see and type keys/);
    await expectAccessible("onboarding: permissions");
  });

  it("steps 3 to 5", async () => {
    const r = onboardingRig({ permissions: NOT_REQUIRED });
    await renderOnboarding(r);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /Get started/ }));
    await onStep(/Make your first snippet/);
    await expectAccessible("onboarding: first snippet");
    await user.type(screen.getByLabelText("Iamb types"), SAMPLE_EMAIL);
    await user.click(screen.getByRole("button", { name: /Save and try it/ }));
    await onStep(/Try it here/);
    await expectAccessible("onboarding: try it");
    personTypes(";em ");
    serviceTypes(`${SAMPLE_EMAIL} `);
    await onStep(/That is all there is to it/);
    await expectAccessible("onboarding: try it, it worked");
    await user.click(await screen.findByRole("button", { name: /Continue/ }));
    await onStep(/Iamb is running/);
    await expectAccessible("onboarding: done");
  });
});

describe("the harness itself", () => {
  it("checks label in name (2.5.3)", async () => {
    document.body.innerHTML = '<button aria-label="Close">Save changes</button><button aria-label="Replies, 3 snippets">Replies <span>3</span></button>';
    expect(labelInName(document.body)).toEqual(['button shows "save changes" but is named "close"']);
  });

  it("fails on a violation (so a pass means something)", async () => {
    document.body.innerHTML = "<button></button>";
    await expect(expectAccessible("canary")).rejects.toThrow(/button-name/);
    delete tally.canary;
    expect(within(document.body).getAllByRole("button")).toHaveLength(1);
    await waitFor(() => {});
  });
});
