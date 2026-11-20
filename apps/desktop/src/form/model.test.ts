import { describe, expect, it } from "vitest";

import type { FieldView } from "../ipc";
import type { FormOpen } from "./api";
import {
  controlOf,
  copy,
  defaults,
  fit,
  initialValues,
  keysFor,
  remembered,
  submission,
  titleOf,
  typeahead,
} from "./model";

const text = (name: string, d = ""): FieldView => ({ name, kind: "text", options: [], default: d });
const area = (name: string, d = ""): FieldView => ({ name, kind: "area", options: [], default: d });
const toggle = (name: string, d = "no"): FieldView => ({
  name,
  kind: "toggle",
  options: [],
  default: d,
});
const choice = (name: string, options: string[], d = ""): FieldView => ({
  name,
  kind: "choice",
  options,
  default: d,
});

const form = (fields: FieldView[], lastValues: Record<string, string> | null = null): FormOpen => ({
  session: 1,
  snippet: "s",
  label: "Late delivery",
  abbreviation: ";rlate",
  app: null,
  fields,
  lastValues,
});

describe("controls", () => {
  it("follow the design system's table (§4.13)", () => {
    expect(controlOf(text("a"))).toBe("input");
    expect(controlOf(area("a"))).toBe("textarea");
    expect(controlOf(toggle("a"))).toBe("checkbox");
    expect(controlOf(choice("a", ["1", "2", "3", "4", "5", "6"]))).toBe("radios");
    expect(controlOf(choice("a", ["1", "2", "3", "4", "5", "6", "7"]))).toBe("select");
  });
});

describe("values", () => {
  it("are corrected to fit their field, never reported (template language §4.5)", () => {
    expect(fit(text("a"), "one\ntwo\r\nthree")).toBe("one two three");
    expect(fit(area("a"), "one\ntwo")).toBe("one\ntwo");
    expect(fit(choice("a", ["S", "M"], "M"), "XL")).toBe("M");
    expect(fit(choice("a", ["S", "M"], "nope"), undefined)).toBe("S");
    expect(fit(choice("a", [], ""), "x")).toBe("");
    expect(fit(toggle("a", "yes"), undefined)).toBe("yes");
    expect(fit(toggle("a"), "true")).toBe("no");
  });

  it("start from the remembered answers, else the defaults", () => {
    const fields = [text("First name", "you"), toggle("Voucher"), choice("Size", ["S", "M"], "M")];
    expect(initialValues(form(fields))).toEqual({ "First name": "you", Voucher: "no", Size: "M" });
    const last = { "First name": "Jun", Voucher: "yes", Size: "S", Other: "x" };
    expect(initialValues(form(fields, last))).toEqual({
      "First name": "Jun",
      Voucher: "yes",
      Size: "S",
    });
    expect(remembered(form(fields, last))).toBe(true);
    expect(remembered(form(fields, { Other: "x" }))).toBe(false);
    expect(remembered(form(fields))).toBe(false);
    expect(defaults(fields)).toEqual({ "First name": "you", Voucher: "no", Size: "M" });
  });

  it("are sent for every field, each made to fit", () => {
    const fields = [text("A"), area("B"), toggle("C"), choice("D", ["x", "y"], "y")];
    expect(submission(fields, { A: "1\n2", B: "3\n4", Extra: "dropped" })).toEqual({
      A: "1 2",
      B: "3\n4",
      C: "no",
      D: "y",
    });
  });
});

describe("type to jump in a list of options", () => {
  const options = ["Road", "Gravel", "City", "Cargo", "Racing"];
  it("finds the next option that starts with what was typed", () => {
    expect(typeahead(options, "Road", "c")).toBe("City");
    expect(typeahead(options, "City", "c")).toBe("Cargo");
    expect(typeahead(options, "Cargo", "c")).toBe("City");
    expect(typeahead(options, "Road", "ca")).toBe("Cargo");
    expect(typeahead(options, "Road", "RA")).toBe("Racing");
    expect(typeahead(options, "City", "cc")).toBe("Cargo");
    expect(typeahead(options, "Road", "z")).toBeUndefined();
    expect(typeahead([], "", "a")).toBeUndefined();
  });
});

describe("keys and copy", () => {
  it("are written as each system writes them", () => {
    expect(keysFor("mac").cancel).toBe("esc");
    expect(keysFor("mac").insert).toBe("⏎");
    expect(keysFor("mac").startEmpty).toBe("⌘⌫");
    expect(keysFor("win").insert).toBe("Enter");
    expect(keysFor("win").startEmpty).toBe("Ctrl+Backspace");
    expect(keysFor("win").aria.insert).toBe("Enter Control+Enter");
  });

  it("is the specification's", () => {
    expect(copy.goesInto("Mail")).toBe("Goes into Mail");
    expect(copy.couldNotGetBack("Mail")).toBe("Iamb could not get back to Mail.");
    expect(copy.nothingTyped).toBe("Nothing was typed. Press Insert to try again, or cancel.");
    expect(copy.stillHere).toBe("Your answers are still here.");
    expect(copy.description(1)).toBe("Fill-in form, 1 field");
    expect(copy.description(4)).toBe("Fill-in form, 4 fields");
    expect(titleOf(form([]))).toBe("Late delivery");
    expect(titleOf({ ...form([]), label: "" })).toBe(";rlate");
  });
});
