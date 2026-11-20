// The sentences against the service: every reason code the service can record has an entry, and
// no entry is for a code the service never records. Read from the Rust sources, so that a code
// added there fails here until it gets a sentence (task 3.9: "make a test fail if a code has no
// sentence").

import { describe, expect, it } from "vitest";

import { KNOWN_CODES, TEMPLATE_ERROR_CODES, reasonOf, render } from "./reasons";

// The sources, as text (the page has no file system): the service's, the engine's reasons, the
// template errors, and the specification.
const sources = import.meta.glob(
  [
    "../../../../../crates/iamb-service/src/**/*.rs",
    "../../../../../crates/iamb-core/src/engine/io.rs",
    "../../../../../crates/iamb-core/src/template/error.rs",
    "../../../../../docs/ux/spec.md",
  ],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

const at = (path: string) => `../../../../../${path}`;
const read = (path: string): string => {
  const text = sources[at(path)];
  if (text === undefined) throw new Error(`${path} is not among the sources this test reads`);
  return text;
};

/** The body of the first `fn code` after `marker`: up to the closing brace at its indent. */
function codeFn(text: string, marker: string): string {
  const from = text.indexOf(marker);
  expect(from, `${marker} in the source`).toBeGreaterThan(-1);
  const rest = text.slice(from);
  const open = rest.indexOf("pub fn code(");
  const end = rest.indexOf("\n    }\n", open);
  return rest.slice(open, end);
}

/** The service's own source files (not its tests). */
function serviceFiles(): string[] {
  return Object.keys(sources)
    .filter((p) => p.includes("/crates/iamb-service/src/"))
    .map((p) => p.slice(at("").length));
}

/** Every code the service or the engine can put in a diagnostic. */
function emitted(): Set<string> {
  const found = new Set<string>();
  // The engine's reasons (`Reason::code`) and the template error kinds (`ErrorKind::code`).
  const reason = codeFn(read("crates/iamb-core/src/engine/io.rs"), "impl Reason {");
  for (const m of reason.matchAll(/"([a-z_]+)"/g)) found.add(m[1] as string);
  const kinds = codeFn(read("crates/iamb-core/src/template/error.rs"), "pub enum ErrorKind");
  for (const m of kinds.matchAll(/"(E_[A-Z_]+)"/g)) found.add(m[1] as string);
  // The service's own: `coded("…")`, and a dropped request's code.
  for (const file of serviceFiles()) {
    const text = read(file);
    for (const m of text.matchAll(/\bcoded\("([a-z_]+)"\)/g)) found.add(m[1] as string);
    for (const m of text.matchAll(/\.dropped\([^;]*?"([a-z_]+)"\)/gs)) found.add(m[1] as string);
  }
  return found;
}

describe("every code the service records has a sentence", () => {
  const codes = emitted();

  it("finds the codes it should (so that it cannot pass by finding none)", () => {
    expect(serviceFiles().length).toBeGreaterThan(10);
    for (const code of [
      "not_a_delimiter",
      "suspended_excluded_app",
      "refused_composing",
      "injection_cancelled",
      "form_no_ui_host",
      "form_not_acknowledged",
      "form_ui_host_gone",
      "panel_focus_expired",
      "keys_for_unreported_window",
      "endpoint_unavailable",
      "request_focus_timeout",
      "request_cancelled",
      "E_UNCLOSED_MACRO",
      "E_OUTPUT_TOO_LARGE",
    ]) {
      expect(codes.has(code), code).toBe(true);
    }
    expect(codes.size).toBeGreaterThanOrEqual(45);
  });

  it("has an entry for each", () => {
    const missing = [...codes].filter((c) => !KNOWN_CODES.includes(c));
    expect(missing, "codes without a sentence in reasons.ts").toEqual([]);
  });

  it("has no entry for a code that is never recorded", () => {
    const stale = KNOWN_CODES.filter((c) => !codes.has(c));
    expect(stale, "entries of reasons.ts the service no longer records").toEqual([]);
  });

  it("gives every template error code its own entry, and the library knows its sentence", () => {
    const kinds = [...codes].filter((c) => c.startsWith("E_")).sort();
    expect([...TEMPLATE_ERROR_CODES].sort()).toEqual(kinds);
  });

  it("writes a sentence for each code, with nothing left over from a template", () => {
    for (const code of KNOWN_CODES) {
      for (const mac of [true, false]) {
        const r = render(code, { label: "Thanks for your order", app: "mybank.exe", mac });
        const words = `${r.title} ${r.detail ?? ""}`;
        expect(words.trim().length, code).toBeGreaterThan(5);
        expect(words, code).not.toMatch(/undefined|null|\$\{|\[object/);
        // A sentence names the snippet's label and the app, and nothing else it was handed.
        expect(r.technical, code).toBe(reasonOf(code).technical === true);
        if (r.technical) expect(r.title).toBe(`Technical: ${code}`);
      }
    }
  });

  it("keeps a template error's sentence the library's, and not the generic one", () => {
    for (const code of TEMPLATE_ERROR_CODES) {
      const r = render(code, { label: "Out of office", app: null, mac: false });
      expect(r.title).toBe("“Out of office” has an error");
      expect(r.detail).not.toContain("This snippet has an error.");
      expect(r.detail?.endsWith("Nothing was typed.")).toBe(true);
      expect(r.action).toBe("open_snippet");
      expect(r.code).toBe(code);
    }
  });
});

describe("the sentences follow the specification's table (§6.2)", () => {
  const spec = read("docs/ux/spec.md");
  const table = spec.slice(spec.indexOf("### 6.2"), spec.indexOf("### 6.3"));

  it("has an entry for each code the table names", () => {
    const named = new Set<string>();
    for (const line of table.split("\n")) {
      if (!line.startsWith("| `")) continue;
      const first = line.split("|")[1] ?? "";
      for (const m of first.matchAll(/`([a-z_]+)`/g)) named.add(m[1] as string);
    }
    expect(named.size).toBeGreaterThanOrEqual(20);
    for (const code of named) expect(KNOWN_CODES, code).toContain(code);
  });

  it("writes the words of the table", () => {
    const ctx = { label: "Late delivery", app: "Notes", mac: true };
    const win = { ...ctx, mac: false };
    const say = (code: string, c = ctx) => {
      const r = render(code, c);
      return [r.title, r.detail].filter(Boolean).join(" · ");
    };
    expect(say("suspended_secure_field")).toBe("Not expanding in a password field · Iamb stores no keys there.");
    expect(say("suspended_excluded_app")).toBe("Not expanding in Notes · You excluded this app.");
    expect(say("suspended_unknown_focus")).toBe("Not expanding in Notes · Iamb cannot see into this window, so it stays out.");
    expect(say("suspended_unknown_focus", win)).toBe(
      "Not expanding in Notes · Iamb cannot see into this window, so it stays out. It may be running as administrator.",
    );
    expect(say("suspended_composing")).toBe("Not expanding while your keyboard composes text · An input method is on.");
    expect(say("refused_secure_field")).toBe("“Late delivery” was not inserted: the cursor was in a password field");
    expect(say("refused_excluded_app")).toBe("“Late delivery” was not inserted: you excluded Notes");
    expect(say("refused_unknown_focus")).toBe("“Late delivery” was not inserted: Iamb cannot see into this window");
    expect(say("refused_own_window")).toBe("“Late delivery” was not inserted into Iamb's own window");
    expect(say("refused_composing")).toBe("“Late delivery” was not inserted: your keyboard was composing text");
    expect(say("request_cancelled")).toBe("“Late delivery” was not inserted: you moved to another app first");
    expect(say("injection_busy")).toBe("“Late delivery” was not inserted: Iamb was still typing the previous snippet");
    expect(say("injection_cancelled")).toBe(
      "“Late delivery” was stopped halfway · You switched windows or pressed a key that moves the cursor while Iamb was typing. What was already typed stayed where it was.",
    );
    expect(say("typed_during_injection")).toBe("Keys you typed while “Late delivery” was being typed may be out of order");
    expect(say("form_open")).toBe("“Late delivery” was not started: another fill-in form was still open");
    expect(say("form_no_ui_host")).toBe(
      "“Late delivery” could not show its form · Iamb's window process was restarting. Type the abbreviation again.",
    );
    expect(say("form_not_acknowledged")).toBe(say("form_no_ui_host"));
    expect(say("no_last_form")).toBe("There is no earlier form to fill in again · Answers are kept for 30 minutes.");
    expect(say("unknown_snippet")).toBe("The snippet no longer exists");
    expect(say("not_a_delimiter")).toBe("“Late delivery” did not expand: Enter and Tab do not count as the end of a word");
  });

  it("offers the buttons of the table", () => {
    const action = (code: string) => render(code, { label: "x", app: null, mac: false }).action;
    expect(action("suspended_excluded_app")).toBe("manage_apps");
    expect(action("not_a_delimiter")).toBe("typing_settings");
    expect(action("E_BAD_SYNTAX")).toBe("open_snippet");
    expect(action("suspended_secure_field")).toBeNull();
    expect(action("refused_excluded_app")).toBeNull();
  });

  it("lists the technical codes of the table only as technical entries, and so is any unknown code", () => {
    for (const code of [
      "keys_for_unreported_window",
      "unexpected_injection_event",
      "unknown_form_session",
      "panel_focus_expired",
      "a_code_of_a_newer_iamb",
    ]) {
      const r = render(code, { label: null, app: null, mac: false });
      expect(r.technical, code).toBe(true);
      expect(r.title).toBe(`Technical: ${code}`);
    }
    expect(render("a_code_of_a_newer_iamb", { label: null, app: null, mac: false }).detail).toBe(
      "This code is not known to this version of Iamb.",
    );
  });

  it("does not name a snippet that is gone", () => {
    const r = render("refused_composing", { label: null, app: null, mac: false });
    expect(r.title).toBe("A snippet was not inserted: your keyboard was composing text");
  });
});
