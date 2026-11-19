#!/usr/bin/env python3
"""Builds the mock-ups of docs/ux/mockups/ and checks the colour contrast of the tokens.

    python3 docs/ux/mockups/build.py            write the seven HTML files
    python3 docs/ux/mockups/build.py --check    fail if a file is out of date or a contrast check fails
    python3 docs/ux/mockups/build.py --contrast print the contrast table of design-system.md

Each page is `src/pages/<name>.html` (the body), wrapped in a shell that inlines, in this order:
the CSS custom properties generated from `../tokens.json`, `src/base.css`, the icon sprite
`src/icons.svg` and `src/base.js`. The result is one self-contained file with no external
resource. Two shorthands in a page: `<!--#include name key=value -->` inserts
`src/partials/name.html` (`sel=id` marks the element with that `data-id` as selected), and
`[[Mod+N]]` becomes the key badge of each system (⌘N on macOS, Ctrl+N on Windows). Standard
library only.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SRC = HERE / "src"
TOKENS = HERE.parent / "tokens.json"

PAGES = [
    ("index", "Iamb mock-ups", "Overview"),
    ("onboarding", "Iamb flow 1: onboarding", "1 Onboarding"),
    ("create-snippet", "Iamb flow 2: create a snippet", "2 Create a snippet"),
    ("expand", "Iamb flow 3: expand", "3 Expand"),
    ("fill-in", "Iamb flow 4: fill-in", "4 Fill-in"),
    ("search", "Iamb flow 5: search", "5 Search"),
    ("share-folder", "Iamb flow 6: share a folder", "6 Share a folder"),
]

# ---------------------------------------------------------------------------------------------
# Tokens → CSS custom properties
# ---------------------------------------------------------------------------------------------


def css_color(value: dict) -> str:
    alpha = value.get("alpha")
    if alpha is None:
        return value["hex"]
    r, g, b = (round(c * 255) for c in value["components"])
    return f"rgb({r} {g} {b} / {alpha})"


def css_dim(value: dict) -> str:
    return f"{value['value']}{value['unit']}"


def css_shadow(value) -> str:
    layers = value if isinstance(value, list) else [value]
    return ", ".join(
        " ".join(
            [css_dim(s["offsetX"]), css_dim(s["offsetY"]), css_dim(s["blur"]), css_dim(s["spread"]), css_color(s["color"])]
        )
        for s in layers
    )


def resolve(tokens: dict, ref: str):
    node = tokens
    for part in ref.strip("{}").split("."):
        node = node[part]
    return node["$value"]


def css_value(tokens: dict, kind: str, value) -> str:
    if kind == "color":
        return css_color(value)
    if kind in ("dimension", "duration"):
        return css_dim(value)
    if kind == "shadow":
        return css_shadow(value)
    if kind == "fontFamily":
        return ", ".join(f'"{f}"' if " " in f else f for f in value)
    if kind == "cubicBezier":
        return "cubic-bezier(" + ", ".join(str(v) for v in value) + ")"
    if kind == "typography":
        family = value["fontFamily"].strip("{}").replace(".", "-")
        return (
            f"{resolve(tokens, value['fontWeight'])} {css_dim(resolve(tokens, value['fontSize']))}"
            f"/{resolve(tokens, value['lineHeight'])} var(--{family})"
        )
    return str(value)


def walk(tokens: dict, node: dict, path: list[str], kind: str | None, out: list[tuple[list[str], str, object]]) -> None:
    kind = node.get("$type", kind)
    if "$value" in node:
        out.append((path, kind or "", node["$value"]))
        return
    for key, child in node.items():
        if not key.startswith("$") and isinstance(child, dict):
            walk(tokens, child, path + [key], kind, out)


def token_css(tokens: dict) -> str:
    flat: list[tuple[list[str], str, object]] = []
    walk(tokens, tokens, [], None, flat)
    shared: list[str] = []
    themed: dict[str, list[str]] = {"light": [], "dark": []}
    for path, kind, value in flat:
        if path[0] in ("color", "shadow") and path[1] in themed:
            name = "-".join([path[0]] + path[2:])
            themed[path[1]].append(f"--{name}: {css_value(tokens, kind, value)};")
            continue
        name = "-".join(path)
        if kind == "typography":
            name = "type-" + "-".join(path[1:])
            shared.append(f"--{name}: {css_value(tokens, kind, value)};")
            tracking = value["letterSpacing"]
            if tracking["value"]:
                shared.append(f"--{name}-tracking: {css_dim(tracking)};")
            continue
        shared.append(f"--{name}: {css_value(tokens, kind, value)};")

    def block(selector: str, lines: list[str], indent: str = "") -> str:
        body = "\n".join(f"{indent}  {line}" for line in lines)
        return f"{indent}{selector} {{\n{body}\n{indent}}}"

    light = shared + themed["light"] + ["color-scheme: light;"]
    dark = themed["dark"] + ["color-scheme: dark;"]
    return "\n".join(
        [
            "/* Generated from docs/ux/tokens.json by docs/ux/mockups/build.py. Do not edit here. */",
            block(":root", light),
            "@media (prefers-color-scheme: dark) {",
            block(':root:not([data-theme="light"])', dark, "  "),
            "}",
            block(':root[data-theme="dark"]', dark),
        ]
    )


# ---------------------------------------------------------------------------------------------
# Contrast (WCAG 2.1 relative luminance)
# ---------------------------------------------------------------------------------------------

# (foreground, background, minimum ratio, what it is used for). 4.5 is WCAG 1.4.3 for text;
# 3.0 is WCAG 1.4.11 for the parts of a control and for graphics that carry meaning.
CHECKS = [
    ("text.primary", "bg.surface", 4.5, "Body text on a surface"),
    ("text.primary", "bg.app", 4.5, "Body text on the window background"),
    ("text.primary", "bg.subtle", 4.5, "Text in a neutral chip or well"),
    ("text.primary", "bg.hover", 4.5, "Text of a hovered row"),
    ("text.primary", "bg.selected", 4.5, "Text of a selected row"),
    ("text.primary", "bg.highlight", 4.5, "Matched characters in search results"),
    ("text.secondary", "bg.surface", 4.5, "Help text and metadata on a surface"),
    ("text.secondary", "bg.app", 4.5, "Help text on the window background"),
    ("text.secondary", "bg.subtle", 4.5, "Keyboard badge"),
    ("text.secondary", "bg.hover", 4.5, "Metadata of a hovered row"),
    ("text.secondary", "bg.selected", 4.5, "Metadata of a selected row"),
    ("text.on-accent", "accent.solid", 4.5, "Primary button label"),
    ("text.on-accent", "accent.solid-hover", 4.5, "Primary button label, hovered"),
    ("text.on-inverse", "bg.inverse", 4.5, "Toast and notice text"),
    ("text.on-danger", "status.danger.solid", 4.5, "Destructive button label"),
    ("accent.text", "bg.surface", 4.5, "Link on a surface"),
    ("accent.text", "bg.app", 4.5, "Link on the window background"),
    ("accent.text", "bg.selected", 4.5, "Link in a selected row"),
    ("status.success.text", "status.success.bg", 4.5, "Success banner and badge"),
    ("status.success.text", "bg.surface", 4.5, "“Granted” in a permission row"),
    ("status.warning.text", "status.warning.bg", 4.5, "Warning banner and badge"),
    ("status.warning.text", "bg.surface", 4.5, "Warning text under a field"),
    ("status.danger.text", "status.danger.bg", 4.5, "Error banner and badge"),
    ("status.danger.text", "bg.surface", 4.5, "Error text under a field"),
    ("macro.insert.text", "macro.insert.bg", 4.5, "Chip: date, time, snippet"),
    ("macro.ask.text", "macro.ask.bg", 4.5, "Chip: field, if, else, endif"),
    ("macro.act.text", "macro.act.bg", 4.5, "Chip: key, wait, cursor"),
    ("macro.read.text", "macro.read.bg", 4.5, "Chip: clipboard"),
    ("border.control", "bg.surface", 3.0, "Input outline on a surface"),
    ("border.control", "bg.app", 3.0, "Input outline on the window background"),
    ("border.control", "bg.subtle", 3.0, "Input outline inside a well"),
    ("border.focus", "bg.surface", 3.0, "Focus ring on a surface"),
    ("border.focus", "bg.app", 3.0, "Focus ring on the window background"),
    ("border.focus", "bg.selected", 3.0, "Focus ring on a selected row"),
    ("accent.solid", "bg.surface", 3.0, "Checked switch, checkbox and radio"),
    ("status.danger.solid", "bg.surface", 3.0, "Destructive button against its surface"),
    ("tag.red", "bg.surface", 3.0, "Tag dot"),
    ("tag.orange", "bg.surface", 3.0, "Tag dot"),
    ("tag.yellow", "bg.surface", 3.0, "Tag dot"),
    ("tag.green", "bg.surface", 3.0, "Tag dot"),
    ("tag.teal", "bg.surface", 3.0, "Tag dot"),
    ("tag.blue", "bg.surface", 3.0, "Tag dot"),
    ("tag.purple", "bg.surface", 3.0, "Tag dot"),
    ("tag.grey", "bg.surface", 3.0, "Tag dot"),
]


def luminance(hex_color: str) -> float:
    h = hex_color.lstrip("#")
    channels = [int(h[i : i + 2], 16) / 255 for i in (0, 2, 4)]
    linear = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def ratio(a: str, b: str) -> float:
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def hex_of(tokens: dict, theme: str, path: str) -> str:
    node = tokens["color"][theme]
    for part in path.split("."):
        node = node[part]
    return node["$value"]["hex"]


def contrast_rows(tokens: dict) -> list[tuple[str, str, float, str, dict[str, tuple[str, str, float]]]]:
    rows = []
    for fg, bg, minimum, use in CHECKS:
        per_theme = {}
        for theme in ("light", "dark"):
            f, b = hex_of(tokens, theme, fg), hex_of(tokens, theme, bg)
            # Floored to two decimals, so a value shown as the minimum really reaches it.
            per_theme[theme] = (f, b, int(ratio(f, b) * 100) / 100)
        rows.append((fg, bg, minimum, use, per_theme))
    return rows


def contrast_table(tokens: dict) -> str:
    lines = [
        "| Foreground | Background | Use | Needs | Light | Dark |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for fg, bg, minimum, use, per in contrast_rows(tokens):
        cells = []
        for theme in ("light", "dark"):
            f, b, r = per[theme]
            cells.append(f"{r:.2f} (`{f}` on `{b}`)")
        lines.append(f"| `{fg}` | `{bg}` | {use} | {minimum:.1f} | {cells[0]} | {cells[1]} |")
    return "\n".join(lines)


def contrast_failures(tokens: dict) -> list[str]:
    bad = []
    for fg, bg, minimum, _use, per in contrast_rows(tokens):
        for theme, (_f, _b, r) in per.items():
            if r < minimum:
                bad.append(f"{theme}: {fg} on {bg} is {r:.2f}, needs {minimum}")
    return bad


# ---------------------------------------------------------------------------------------------
# Pages
# ---------------------------------------------------------------------------------------------

INCLUDE = re.compile(r"<!--#include (\S+?)((?: \S+?=\S+?)*) -->")
KEYS = re.compile(r"\[\[([^\]\[]+)\]\]")
# Key names in `[[Mod+N]]`: (macOS, Windows). A chord is written the way each system writes it.
KEY_NAMES = {
    "Mod": ("⌘", "Ctrl"), "Ctrl": ("⌃", "Ctrl"), "Alt": ("⌥", "Alt"), "Shift": ("⇧", "Shift"),
    "Enter": ("⏎", "Enter"), "Esc": ("esc", "Esc"), "Tab": ("⇥", "Tab"), "Backspace": ("⌫", "Backspace"),
    "Up": ("↑", "↑"), "Down": ("↓", "↓"), "Left": ("←", "←"), "Right": ("→", "→"),
}


def expand_keys(text: str) -> str:
    def replace(match: re.Match) -> str:
        parts = match.group(1).split("+")
        mac = "".join(KEY_NAMES.get(p, (p, p))[0] for p in parts)
        win = "+".join(KEY_NAMES.get(p, (p, p))[1] for p in parts)
        if mac == win:
            return f"<kbd>{mac}</kbd>"
        return f'<kbd class="only-mac">{mac}</kbd><kbd class="only-win">{win}</kbd>'

    return KEYS.sub(replace, text)
SHELL = """<!doctype html>
<html lang="en" data-os="mac">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<!-- Built by docs/ux/mockups/build.py from src/pages/{name}.html. Edit the sources, then run the script. -->
<style>
{tokens}
{css}
</style>
</head>
<body>
{sprite}
<header class="page-head">
  <a class="brand" href="index.html" aria-label="Iamb mock-ups, overview"><svg class="i" aria-hidden="true"><use href="#i-iamb"/></svg><span>Iamb</span></a>
  <nav aria-label="Flows">
{nav}
  </nav>
  <div class="switches">
    <div class="seg" role="group" aria-label="Theme">
      <button type="button" data-set-theme="system" aria-pressed="true">System</button>
      <button type="button" data-set-theme="light" aria-pressed="false">Light</button>
      <button type="button" data-set-theme="dark" aria-pressed="false">Dark</button>
    </div>
    <div class="seg" role="group" aria-label="Window chrome">
      <button type="button" data-set-os="mac" aria-pressed="true">macOS</button>
      <button type="button" data-set-os="win" aria-pressed="false">Windows</button>
    </div>
  </div>
</header>
<main>
{body}
</main>
<footer class="page-foot">Mock-up for review (task 3.1). Static HTML: nothing here is wired to the service. Sample names, addresses and apps are invented.</footer>
<script>
{js}
</script>
</body>
</html>
"""


def expand_includes(text: str) -> str:
    def replace(match: re.Match) -> str:
        part = (SRC / "partials" / f"{match.group(1)}.html").read_text(encoding="utf-8").rstrip("\n")
        for pair in match.group(2).split():
            key, value = pair.split("=", 1)
            if key == "sel":
                part = part.replace(f'data-id="{value}"', f'data-id="{value}" aria-selected="true"')
            else:
                part = part.replace("{{" + key + "}}", value.replace("_", " "))
        return part

    return INCLUDE.sub(replace, text)


def build(tokens: dict) -> dict[Path, str]:
    css = (SRC / "base.css").read_text(encoding="utf-8").rstrip("\n")
    js = (SRC / "base.js").read_text(encoding="utf-8").rstrip("\n")
    sprite = (SRC / "icons.svg").read_text(encoding="utf-8").rstrip("\n")
    variables = token_css(tokens)
    out = {}
    for name, title, _label in PAGES:
        nav = "\n".join(
            f'    <a href="{n}.html"{" aria-current=" + chr(34) + "page" + chr(34) if n == name else ""}>{label}</a>'
            for n, _t, label in PAGES
        )
        body = expand_keys(expand_includes((SRC / "pages" / f"{name}.html").read_text(encoding="utf-8"))).rstrip("\n")
        out[HERE / f"{name}.html"] = SHELL.format(
            title=title, name=name, tokens=variables, css=css, sprite=sprite, nav=nav, body=body, js=js
        )
    return out


def main() -> int:
    tokens = json.loads(TOKENS.read_text(encoding="utf-8"))
    if "--contrast" in sys.argv:
        print(contrast_table(tokens))
        return 0
    failures = contrast_failures(tokens)
    for line in failures:
        print(f"contrast: {line}", file=sys.stderr)
    pages = build(tokens)
    if "--check" in sys.argv:
        stale = [p.name for p, text in pages.items() if not p.exists() or p.read_text(encoding="utf-8") != text]
        for name in stale:
            print(f"out of date: {name} (run docs/ux/mockups/build.py)", file=sys.stderr)
        return 1 if failures or stale else 0
    for path, text in pages.items():
        path.write_text(text, encoding="utf-8")
    print(f"wrote {len(pages)} files; {len(CHECKS) * 2} contrast checks, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
