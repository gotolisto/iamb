// WCAG 2.5.3 Label in name (docs/ux/spec.md §8: "the accessible name starts with the visible
// label"). axe's own rule for it is experimental and compares the whole visible text with the
// name, which flags a name such as "Replies, 3 snippets, level 1" for a button that shows
// "Replies" and a count. The commitment is the stricter one about the *start*: speech control
// says "click Replies". This function is self-contained: vitest calls it in jsdom, and the
// keyboard-only run sends its source into Chromium.

/**
 * The elements under `root` that have an `aria-label` and visible text
 * of their own, whose accessible name does not start with the first words of that text.
 * Returns what is wrong, as strings.
 */
export function labelInName(root = document.body) {
  const norm = (t) =>
    t
      .toLowerCase()
      .replace(/[​-‏⁠]/g, "")
      .replace(/[^\p{L}\p{N}]+/gu, " ")
      .trim();
  /** The text a person sees on `el`, in reading order, without what is hidden. */
  const visible = (el) => {
    let out = "";
    const walk = (n) => {
      if (n.nodeType === 3) out += n.textContent;
      else if (n.nodeType === 1) {
        if (n.getAttribute("aria-hidden") === "true" || n.hidden) return;
        const s = n.ownerDocument.defaultView.getComputedStyle(n);
        if (s.display === "none" || s.visibility === "hidden") return;
        if (n.classList.contains("visually-hidden") || n.classList.contains("sr")) return;
        for (const c of n.childNodes) walk(c);
        out += " ";
      }
    };
    for (const c of el.childNodes) walk(c);
    return out;
  };
  const nameOf = (el) => el.getAttribute("aria-label") ?? "";
  const wrong = [];
  // 2.5.3 is about controls (landmarks, lists and groups are named by their own label).
  const controls = 'button, a[href], summary, [role="button"], [role="link"], [role="menuitem"], [role="option"], [role="treeitem"], [role="tab"], [role="switch"], [role="checkbox"], [role="radio"]';
  for (const el of root.querySelectorAll(controls)) {
    // Named by an element on the page (`aria-labelledby`): that element is the visible label, and
    // what the control itself shows is its value (a switch's "On", a recorded shortcut).
    if (!el.hasAttribute("aria-label") || el.hasAttribute("aria-labelledby")) continue;
    if (el.getAttribute("aria-hidden") === "true" || el.closest("[aria-hidden='true']")) continue;
    // Inputs and the like show their label elsewhere (a <label>); only controls with their own text.
    if (/^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName)) continue;
    const text = norm(visible(el));
    if (!text) continue;
    const name = norm(nameOf(el));
    if (!name) continue;
    // The name must start with the first words of the visible text (the first three, so that a
    // count or a key badge after the label is not part of the comparison).
    const head = text.split(" ").slice(0, 3).join(" ");
    const label = text.split(" ").slice(0, 1).join(" ");
    if (!(name.startsWith(head) || name.startsWith(label))) {
      wrong.push(`${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""} shows "${text.slice(0, 40)}" but is named "${name.slice(0, 40)}"`);
    }
  }
  return wrong;
}
