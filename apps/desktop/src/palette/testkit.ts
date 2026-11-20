// A fake Rust side for the palette's tests: the commands, recorded, and the panel's event.
// Invented library data; nothing here is a user's text.

import type { PaletteHit, PaletteOpen, PaletteRow } from "./api";

export function row(over: Partial<PaletteRow> = {}): PaletteRow {
  return {
    id: "01010101-0101-0101-0101-010101010101",
    abbreviation: ";rret",
    label: "Return label",
    folder: "Replies › Returns",
    firstLine: "You can return any part within 30 days.",
    kind: "plain",
    enabled: true,
    broken: null,
    needsReview: false,
    tags: [],
    ...over,
  };
}

let next = 0;
export function hit(over: Partial<PaletteRow> = {}, rest: Partial<PaletteHit> = {}): PaletteHit {
  next += 1;
  const id = `0000000${next}-0101-0101-0101-010101010101`;
  return { row: row({ id, ...over }), field: "label", ranges: [], ...rest };
}

export function openOf(over: Partial<PaletteOpen> = {}): PaletteOpen {
  return { session: 4, ownWindow: false, app: null, lastForm: null, ...over };
}
