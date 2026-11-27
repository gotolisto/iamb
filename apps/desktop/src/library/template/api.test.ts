import { describe, expect, it } from "vitest";

import fixtures from "../../contract.fixtures.json";
import { COMMANDS } from "../../ipc";
import {
  createTemplateService,
  isParsedTemplate,
  isTemplatePreview,
  TEMPLATE_COMMANDS,
  type TemplateCommand,
} from "./api";

describe("the template commands", () => {
  it("are registered, and called by name with their arguments", async () => {
    for (const c of TEMPLATE_COMMANDS) expect(COMMANDS).toContain(c);
    const calls: Array<{ command: TemplateCommand; args: Record<string, unknown> }> = [];
    const service = createTemplateService((command, args) => {
      calls.push({ command, args });
      return Promise.resolve(command === "template_parse" ? fixtures.template.parsed : fixtures.template.preview);
    });
    await service.parse("Hi {field name=Name}");
    await service.preview("Hi {field name=Name}");
    await service.preview("x", { Name: "Ada" });
    expect(calls).toEqual([
      { command: "template_parse", args: { source: "Hi {field name=Name}" } },
      { command: "template_preview", args: { source: "Hi {field name=Name}", values: null } },
      { command: "template_preview", args: { source: "x", values: { Name: "Ada" } } },
    ]);
  });

  it("try the form with the fields, and answer its values or null", async () => {
    const calls: Array<{ command: TemplateCommand; args: Record<string, unknown> }> = [];
    let answer: unknown = { Name: "Ada" };
    const service = createTemplateService((command, args) => {
      calls.push({ command, args });
      return Promise.resolve(answer);
    });
    const fields = [{ name: "Name", kind: "text" as const, options: [], default: "" }];
    expect(await service.tryForm?.(fields, "Greeting", "hi")).toEqual({ Name: "Ada" });
    expect(calls).toEqual([
      { command: "template_try_form", args: { fields, label: "Greeting", abbreviation: "hi" } },
    ]);
    answer = null;
    expect(await service.tryForm?.(fields, "", "hi")).toBeNull();
    answer = { Name: 3 };
    await expect(service.tryForm?.(fields, "", "hi")).rejects.toMatchObject({ code: "unexpected_response" });
  });

  it("open a link by its address only, and pass the Rust side's refusal on", async () => {
    const calls: Array<{ command: TemplateCommand; args: Record<string, unknown> }> = [];
    let fail = false;
    const service = createTemplateService((command, args) => {
      calls.push({ command, args });
      return fail ? Promise.reject(fixtures.template.link_refused) : Promise.resolve(null);
    });
    await service.openLink?.("https://example.com/a?b=1&c=2");
    expect(calls).toEqual([{ command: "template_open_link", args: { url: "https://example.com/a?b=1&c=2" } }]);
    fail = true;
    await expect(service.openLink?.("javascript:alert(1)")).rejects.toMatchObject({ code: "link_refused" });
  });

  it("refuse an answer of the wrong shape", async () => {
    const service = createTemplateService(() => Promise.resolve({ tokens: "nope" }));
    await expect(service.parse("x")).rejects.toMatchObject({ code: "unexpected_response" });
    await expect(service.preview("x")).rejects.toMatchObject({ code: "unexpected_response" });
  });
});

describe("payloads written by the Rust side (contract.fixtures.json, template)", () => {
  it("are accepted by the guards", () => {
    expect(isParsedTemplate(fixtures.template.parsed)).toBe(true);
    expect(isTemplatePreview(fixtures.template.preview)).toBe(true);
    expect(fixtures.template.preview.parts.map((p) => p.part)).toEqual(["text", "key", "wait", "formatted"]);
    // The golden attachment has no file on this machine: the image shows as its alt text.
    expect(fixtures.template.preview.parts[3]).toEqual({
      part: "formatted",
      html: "<p><strong>Hi</strong> logo</p>",
      plain: "Hi logo",
    });
  });

  it("are rejected when their shape changes", () => {
    const parsed = fixtures.template.parsed;
    expect(isParsedTemplate({ ...parsed, tokens: [{ kind: "nope", start: 0, end: 1, attributes: [] }] })).toBe(false);
    expect(isParsedTemplate({ ...parsed, error: { code: "E_X", start: "1", end: null, snippet: null } })).toBe(false);
    const preview = fixtures.template.preview;
    expect(isTemplatePreview({ ...preview, parts: [{ part: "text", text: "x" }] })).toBe(false);
    expect(isTemplatePreview({ ...preview, cursor: { part: -1, offset: 0 } })).toBe(false);
    expect(isTemplatePreview({ ...preview, fields: [{ name: "x" }] })).toBe(false);
    expect(isTemplatePreview({ ...preview, parts: [{ part: "formatted", html: 1, plain: "" }] })).toBe(false);
  });
});

describe("template.css", () => {
  it("uses only design tokens that exist", async () => {
    // Read from disk: Vitest replaces imported CSS with nothing. (No Node types here, hence the
    // loose import.)
    const fs = (await import(/* @vite-ignore */ `node:${"fs"}`)) as {
      readFileSync: (path: URL, encoding: "utf8") => string;
    };
    const tokensCss = fs.readFileSync(new URL("../../styles/tokens.css", import.meta.url), "utf8");
    const templateCss = fs.readFileSync(new URL("./template.css", import.meta.url), "utf8");
    const defined = new Set(tokensCss.match(/--[a-z0-9-]+(?=\s*:)/g) ?? []);
    const used = new Set(templateCss.match(/var\((--[a-z0-9-]+)/g)?.map((v: string) => v.slice(4)) ?? []);
    expect([...used].filter((t) => !defined.has(t))).toEqual([]);
    expect(used.size).toBeGreaterThan(20);
  });
});
