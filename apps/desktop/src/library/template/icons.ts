// The icons the template editor uses, from the design system's set (docs/ux/mockups/src/
// icons.svg: 16 × 16, 1.5 px strokes, round caps, currentColor). Built as DOM nodes because the
// editor's text box is drawn without React (see `dom.ts`); `Icon` in `ui.tsx` draws the same
// paths in React.

import type { IconName } from "./macros";

type Shape = ["path", string, string?] | ["rect", number, number, number, number, number] | ["circle", number, number, number];

export const ICONS: Record<IconName, Shape[]> = {
  date: [["rect", 2, 3, 12, 11, 1.5], ["path", "M2 6.5h12M5 1.8v2.4M11 1.8v2.4"]],
  time: [["circle", 8, 8, 6], ["path", "M8 4.5V8l2.5 1.5"]],
  clipboard: [["rect", 3.5, 3, 9, 11, 1], ["rect", 6, 1.8, 4, 2.4, 0.8]],
  cursor: [["path", "M5.5 2.5c1.5 0 2.5.5 2.5 1.5v8c0 1-1 1.5-2.5 1.5M10.5 2.5C9 2.5 8 3 8 4M10.5 13.5C9 13.5 8 13 8 12M6.5 8h3"]],
  field: [["rect", 1.5, 4.5, 13, 7, 1.5], ["path", "M4.5 7v2"]],
  area: [["path", "M2.5 4h11M2.5 8h11M2.5 12h7"]],
  choice: [["circle", 3.5, 4.5, 1.3], ["circle", 3.5, 11.5, 1.3], ["path", "M7 4.5h7M7 11.5h7"]],
  toggle: [["rect", 1.5, 4.5, 13, 7, 3.5], ["circle", 10.8, 8, 1.6]],
  key: [["path", "M12.5 3.5v4a1.5 1.5 0 0 1-1.5 1.5H4M6.5 6.2 3.7 9l2.8 2.8"]],
  wait: [["path", "M4.5 2.5h7M4.5 13.5h7M5.5 2.5v2.2L8 8l2.5-3.3V2.5M5.5 13.5v-2.2L8 8l2.5 3.3v2.2"]],
  snippet: [
    [
      "path",
      "M6 2.5c-1.5 0-2 .7-2 2v1.7c0 .9-.5 1.5-1.5 1.8 1 .3 1.5.9 1.5 1.8v1.7c0 1.3.5 2 2 2M10 2.5c1.5 0 2 .7 2 2v1.7c0 .9.5 1.5 1.5 1.8-1 .3-1.5.9-1.5 1.8v1.7c0 1.3-.5 2-2 2",
    ],
  ],
  if: [["path", "M4 2.5v11M12 13.5V11c0-2.5-8-2-8-5"]],
  down: [["path", "M3.5 6 8 10.5 12.5 6"]],
  warn: [["path", "M8 2.2 14.3 13H1.7z"], ["path", "M8 6.5v3M8 11.3v.1"]],
};

const SVG = "http://www.w3.org/2000/svg";

/** The icon as an SVG element, hidden from assistive technology (it sits beside a label). */
export function iconElement(doc: Document, name: IconName): SVGSVGElement {
  const svg = doc.createElementNS(SVG, "svg");
  svg.setAttribute("class", "tpl-icon");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  for (const shape of ICONS[name]) {
    const el = doc.createElementNS(SVG, shape[0]);
    if (shape[0] === "path") {
      el.setAttribute("d", shape[1]);
    } else if (shape[0] === "rect") {
      el.setAttribute("x", String(shape[1]));
      el.setAttribute("y", String(shape[2]));
      el.setAttribute("width", String(shape[3]));
      el.setAttribute("height", String(shape[4]));
      el.setAttribute("rx", String(shape[5]));
    } else {
      el.setAttribute("cx", String(shape[1]));
      el.setAttribute("cy", String(shape[2]));
      el.setAttribute("r", String(shape[3]));
    }
    svg.append(el);
  }
  return svg;
}
