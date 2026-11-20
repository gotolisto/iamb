// For tests only: a transcription of the service's tokenizer (`iamb-core`,
// `template/syntax.rs`, `tokenize` and `tokens`), producing what `ParseTemplate` answers as
// tokens. The editor never uses it: chips come from the service (ADR-0017). The tests use it as
// an oracle for random edits, and check it against the service's own answers for every example
// of the template language (`spec.fixtures.json`).

import type { MacroName, TemplateToken } from "./api";

const ALLOWS: Record<MacroName, readonly string[]> = {
  date: ["fmt", "shift"],
  time: ["fmt", "shift"],
  clipboard: ["trim"],
  snippet: ["abbr"],
  field: ["name", "type", "default", "options", "on", "off"],
  key: ["name", "shift", "times"],
  wait: ["ms"],
  if: ["field", "is", "not"],
  cursor: [],
  else: [],
  endif: [],
  lbrace: [],
};

const isWs = (c: string | undefined) => c === " " || c === "\t" || c === "\n" || c === "\r";
const isBare = (c: string | undefined) => c !== undefined && /^[A-Za-z0-9_+\-.%:/]$/.test(c);
const isLower = (c: string | undefined) => c !== undefined && c >= "a" && c <= "z";

class Malformed extends Error {}

/** The tokens of `src`, or none when it does not tokenize (as the service answers). */
export function referenceTokens(src: string): TemplateToken[] {
  try {
    return tokenize(src);
  } catch (e) {
    if (e instanceof Malformed) return [];
    throw e;
  }
}

function lowerRun(src: string, at: number): number {
  while (isLower(src[at])) at++;
  return at;
}

function tokenize(src: string): TemplateToken[] {
  const out: TemplateToken[] = [];
  const len = src.length;
  let textStart = 0;
  let at = 0;
  while (at < len) {
    const off = src.indexOf("{", at);
    if (off < 0) break;
    at = off;
    const nameEnd = lowerRun(src, at + 1);
    const name = src.slice(at + 1, nameEnd);
    if (!(name in ALLOWS)) {
      at = Math.max(nameEnd, at + 1);
      continue;
    }
    if (nameEnd >= len) throw new Malformed();
    if (src[nameEnd] !== "}" && !isWs(src[nameEnd])) {
      at = nameEnd;
      continue;
    }
    if (textStart < at) out.push({ kind: "text", start: textStart, end: at, attributes: [] });
    const macro = name as MacroName;
    const { attributes, end } = attrs(src, nameEnd, macro);
    out.push({ kind: macro, start: at, end, attributes });
    at = end;
    textStart = end;
  }
  if (textStart < len) out.push({ kind: "text", start: textStart, end: len, attributes: [] });
  return out;
}

function attrs(src: string, at: number, name: MacroName) {
  const len = src.length;
  const attributes: Array<{ name: string; value: string }> = [];
  for (;;) {
    while (at < len && isWs(src[at])) at++;
    if (at >= len) throw new Malformed();
    if (src[at] === "}") return { attributes, end: at + 1 };
    const keyEnd = lowerRun(src, at);
    if (keyEnd >= len) throw new Malformed();
    if (keyEnd === at || src[keyEnd] !== "=") throw new Malformed();
    const key = src.slice(at, keyEnd);
    at = keyEnd + 1;
    if (at >= len) throw new Malformed();
    let value: string;
    if (src[at] === '"') {
      let i = at + 1;
      value = "";
      for (;;) {
        if (i >= len) throw new Malformed();
        const c = src[i];
        if (c === "\\" && (src[i + 1] === '"' || src[i + 1] === "\\")) {
          value += src[i + 1];
          i += 2;
        } else if (c === '"') {
          break;
        } else {
          value += c;
          i++;
        }
      }
      at = i + 1;
    } else {
      let end = at;
      while (end < len && isBare(src[end])) end++;
      if (end === at) throw new Malformed();
      value = src.slice(at, end);
      at = end;
    }
    if (at < len && !isWs(src[at]) && src[at] !== "}") throw new Malformed();
    if (!ALLOWS[name].includes(key)) throw new Malformed();
    if (attributes.some((a) => a.name === key)) throw new Malformed();
    attributes.push({ name: key, value: value.replace(/\r\n?/g, "\n") });
  }
}
