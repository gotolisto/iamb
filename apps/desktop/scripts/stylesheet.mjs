// Reads a stylesheet of src/ for the front end's tests (they run without Node's types): the
// custom properties it uses, and those tokens.css defines. `src/form/styles.test.ts` checks the
// form panel's stylesheet with it, as `src/styles/tokens.test.ts` checks base.css.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** The text of `src/<path>`. */
export function stylesheet(path) {
  return readFileSync(join(here, "../src", path), "utf8");
}

/** The custom properties `src/<path>` uses that tokens.css does not define, and how many it uses. */
export function undefinedTokens(path) {
  const defined = new Set(
    [...stylesheet("styles/tokens.css").matchAll(/(--[a-z0-9-]+):/g)].map((m) => m[1]),
  );
  const used = new Set([...stylesheet(path).matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]));
  return { used: used.size, missing: [...used].filter((t) => !defined.has(t)) };
}
