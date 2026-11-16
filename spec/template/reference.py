#!/usr/bin/env python3
"""Reference implementation of the Iamb template language (docs/template-language.md).

This is the oracle used to check the example files in spec/template/examples/. It is written for
clarity, not speed, and is not shipped. The production implementation lives in iamb-core.

Usage: python3 spec/template/reference.py [--min-per-macro N] [examples-dir]
Exits 0 when every example matches and the coverage rules hold, 1 otherwise.
"""

from __future__ import annotations

import datetime as dt
import pathlib
import sys
import tomllib

RESERVED = {
    "date", "time", "clipboard", "cursor", "snippet", "field", "key", "wait",
    "if", "else", "endif", "lbrace",
}
MACRO_GROUPS = ["date", "time", "clipboard", "cursor", "snippet", "field", "key", "wait", "if",
                "syntax"]
KEYS = {
    "enter", "tab", "escape", "backspace", "delete", "space", "up", "down", "left", "right",
    "home", "end", "pageup", "pagedown",
}
ATTRS = {
    "date": {"fmt", "shift"},
    "time": {"fmt", "shift"},
    "clipboard": {"trim"},
    "cursor": set(),
    "snippet": {"abbr"},
    "field": {"name", "type", "default", "options", "on", "off"},
    "key": {"name", "shift", "times"},
    "wait": {"ms"},
    "if": {"field", "is", "not"},
    "else": set(),
    "endif": set(),
    "lbrace": set(),
}
MAX_DEPTH = 8
MAX_WAIT_TOTAL = 30_000
MAX_OUTPUT = 100_000
BARE = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_+-.%:/")
WS = " \t\n"

NAMES = {
    "en": (
        ["January", "February", "March", "April", "May", "June", "July", "August", "September",
         "October", "November", "December"],
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
        ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    ),
    "es": (
        ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre",
         "octubre", "noviembre", "diciembre"],
        ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
        ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
        ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
    ),
    "de": (
        ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September",
         "Oktober", "November", "Dezember"],
        ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
        ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"],
        ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
    ),
    "fr": (
        ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre",
         "octobre", "novembre", "décembre"],
        ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
         "nov.", "déc."],
        ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
        ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
    ),
}


class TemplateError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def normalize_newlines(s: str) -> str:
    return s.replace("\r\n", "\n").replace("\r", "\n")


# ---------------------------------------------------------------- parsing

def tokenize(src: str):
    """Yield ("text", str) and ("macro", name, attrs) tokens."""
    src = normalize_newlines(src)
    out, buf, i, n = [], [], 0, len(src)
    while i < n:
        c = src[i]
        if c != "{":
            buf.append(c)
            i += 1
            continue
        j = i + 1
        while j < n and src[j].islower() and src[j].isascii():
            j += 1
        name = src[i + 1:j]
        if name not in RESERVED or j >= n or (src[j] != "}" and src[j] not in WS):
            if name in RESERVED and j >= n:
                raise TemplateError("E_UNCLOSED_MACRO")
            buf.append(c)
            i += 1
            continue
        if buf:
            out.append(("text", "".join(buf)))
            buf = []
        attrs, i = parse_attrs(src, j, name)
        out.append(("macro", name, attrs))
    if buf:
        out.append(("text", "".join(buf)))
    return out


def parse_attrs(src: str, i: int, macro: str):
    attrs, n = {}, len(src)
    while True:
        while i < n and src[i] in WS:
            i += 1
        if i >= n:
            raise TemplateError("E_UNCLOSED_MACRO")
        if src[i] == "}":
            return attrs, i + 1
        j = i
        while j < n and src[j].islower() and src[j].isascii():
            j += 1
        key = src[i:j]
        if not key or j >= n or src[j] != "=":
            raise TemplateError("E_UNCLOSED_MACRO" if j >= n else "E_BAD_SYNTAX")
        i = j + 1
        if i >= n:
            raise TemplateError("E_UNCLOSED_MACRO")
        if src[i] == '"':
            i += 1
            val = []
            while True:
                if i >= n:
                    raise TemplateError("E_UNCLOSED_MACRO")
                ch = src[i]
                if ch == "\\" and i + 1 < n and src[i + 1] in '"\\':
                    val.append(src[i + 1])
                    i += 2
                elif ch == '"':
                    i += 1
                    break
                else:
                    val.append(ch)
                    i += 1
            value = "".join(val)
        else:
            j = i
            while j < n and src[j] in BARE:
                j += 1
            if j == i:
                raise TemplateError("E_UNCLOSED_MACRO" if j >= n else "E_BAD_SYNTAX")
            value = src[i:j]
            i = j
        if i < n and src[i] not in WS and src[i] != "}":
            raise TemplateError("E_BAD_SYNTAX")
        if key not in ATTRS[macro]:
            raise TemplateError("E_UNKNOWN_ATTR")
        if key in attrs:
            raise TemplateError("E_DUPLICATE_ATTR")
        attrs[key] = value


def yes_no(v: str) -> bool:
    if v not in ("yes", "no"):
        raise TemplateError("E_BAD_VALUE")
    return v == "yes"


def int_in(v: str, lo: int, hi: int) -> int:
    if not (v.isascii() and v.isdigit()) or len(v) > 6 or not lo <= int(v) <= hi:
        raise TemplateError("E_BAD_VALUE")
    return int(v)


def validate(name: str, a: dict) -> dict:
    """Check one macro's attributes; return the normalised node."""
    def need(k):
        if k not in a:
            raise TemplateError("E_MISSING_ATTR")
        if a[k] == "":
            raise TemplateError("E_BAD_VALUE")
        return a[k]

    node = {"kind": name}
    if name in ("date", "time"):
        node["fmt"] = parse_format(a["fmt"]) if "fmt" in a else None
        node["shift"] = parse_shift(a.get("shift", ""))
    elif name == "clipboard":
        node["trim"] = yes_no(a.get("trim", "no"))
    elif name == "snippet":
        node["abbr"] = need("abbr")
    elif name == "field":
        fname = need("name")
        if len(fname) > 64 or "\n" in fname:
            raise TemplateError("E_BAD_VALUE")
        ftype = a.get("type", "text")
        if ftype not in ("text", "area", "choice", "toggle"):
            raise TemplateError("E_BAD_VALUE")
        allowed = {"name", "type", "default"} | {
            "choice": {"options"}, "toggle": {"on", "off"}}.get(ftype, set())
        if set(a) - allowed:
            raise TemplateError("E_UNKNOWN_ATTR")
        node.update(name=fname, type=ftype)
        if ftype == "choice":
            if "options" not in a:
                raise TemplateError("E_MISSING_ATTR")
            opts = a["options"].split("|")
            if any(o == "" for o in opts) or len(set(opts)) != len(opts):
                raise TemplateError("E_BAD_VALUE")
            node["options"] = opts
            node["default"] = a.get("default", opts[0])
            if node["default"] not in opts:
                raise TemplateError("E_BAD_VALUE")
        elif ftype == "toggle":
            node["default"] = "yes" if yes_no(a.get("default", "no")) else "no"
            node["on"], node["off"] = a.get("on", ""), a.get("off", "")
        else:
            node["default"] = normalize_newlines(a.get("default", ""))
            if ftype == "text" and "\n" in node["default"]:
                raise TemplateError("E_BAD_VALUE")
    elif name == "key":
        k = need("name")
        if k not in KEYS:
            raise TemplateError("E_BAD_VALUE")
        node.update(name=k, shift=yes_no(a.get("shift", "no")),
                    times=int_in(a.get("times", "1"), 1, 50))
    elif name == "wait":
        node["ms"] = int_in(need("ms"), 1, 10_000)
    elif name == "if":
        node["field"] = need("field")
        if "is" in a and "not" in a:
            raise TemplateError("E_BAD_VALUE")
        node["op"] = "is" if "is" in a else "not" if "not" in a else "truthy"
        node["value"] = a.get("is", a.get("not"))
    return node


def parse(src: str):
    """Return a tree: list of str | node dict. `if` nodes carry `then` and `otherwise` lists."""
    root: list = []
    stack = [(root, None)]  # (current list, open if-node)
    for tok in tokenize(src):
        cur = stack[-1][0]
        if tok[0] == "text":
            cur.append(tok[1])
            continue
        node = validate(tok[1], tok[2])
        kind = node["kind"]
        if kind == "lbrace":
            cur.append("{")
        elif kind == "if":
            if len(stack) > MAX_DEPTH:
                raise TemplateError("E_TOO_DEEP")
            node["then"], node["otherwise"], node["in_else"] = [], [], False
            cur.append(node)
            stack.append((node["then"], node))
        elif kind == "else":
            open_if = stack[-1][1]
            if open_if is None or open_if["in_else"]:
                raise TemplateError("E_UNBALANCED_IF")
            open_if["in_else"] = True
            stack[-1] = (open_if["otherwise"], open_if)
        elif kind == "endif":
            if stack[-1][1] is None:
                raise TemplateError("E_UNBALANCED_IF")
            stack.pop()
        else:
            cur.append(node)
    if len(stack) != 1:
        raise TemplateError("E_UNBALANCED_IF")
    return root


# ---------------------------------------------------------------- dates

FORMAT_SPECS = ["%-m", "%-d", "%-H", "%-I", "%Y", "%y", "%m", "%d", "%B", "%b", "%A", "%a", "%H",
                "%I", "%M", "%S", "%p", "%j", "%%"]


def parse_format(fmt: str):
    if fmt == "":
        raise TemplateError("E_BAD_VALUE")
    parts, i = [], 0
    while i < len(fmt):
        if fmt[i] != "%":
            parts.append(fmt[i])
            i += 1
            continue
        for spec in FORMAT_SPECS:
            if fmt.startswith(spec, i):
                parts.append(spec)
                i += len(spec)
                break
        else:
            raise TemplateError("E_BAD_FORMAT")
    return parts


UNITS = ["min", "mo", "bd", "d", "w", "y", "h", "s"]


def parse_shift(s: str):
    terms = []
    for term in s.split():
        if term[0] not in "+-":
            raise TemplateError("E_BAD_SHIFT")
        body = term[1:]
        unit = next((u for u in UNITS if body.endswith(u)), None)
        digits = body[: len(body) - len(unit)] if unit else ""
        if not unit or not (digits.isascii() and digits.isdigit()) or len(digits) > 4:
            raise TemplateError("E_BAD_SHIFT")
        terms.append((int(digits) * (1 if term[0] == "+" else -1), unit))
    return terms


def add_months(d: dt.datetime, months: int) -> dt.datetime:
    idx = d.year * 12 + (d.month - 1) + months
    year, month = divmod(idx, 12)
    month += 1
    if not 1 <= year <= 9999:
        raise TemplateError("E_DATE_RANGE")
    nxt = dt.date(year + (month == 12), month % 12 + 1, 1) if (year, month) != (9999, 12) else None
    last = (nxt - dt.timedelta(days=1)).day if nxt else 31
    return d.replace(year=year, month=month, day=min(d.day, last))


def apply_shift(d: dt.datetime, terms) -> dt.datetime:
    try:
        for n, unit in terms:
            if unit == "d":
                d += dt.timedelta(days=n)
            elif unit == "w":
                d += dt.timedelta(weeks=n)
            elif unit == "mo":
                d = add_months(d, n)
            elif unit == "y":
                d = add_months(d, 12 * n)
            elif unit == "h":
                d += dt.timedelta(hours=n)
            elif unit == "min":
                d += dt.timedelta(minutes=n)
            elif unit == "s":
                d += dt.timedelta(seconds=n)
            elif unit == "bd":
                step = dt.timedelta(days=1 if n > 0 else -1)
                left = abs(n)
                while left:
                    d += step
                    if d.weekday() < 5:
                        left -= 1
    except OverflowError:
        raise TemplateError("E_DATE_RANGE") from None
    return d


def format_date(d: dt.datetime, parts, locale: str) -> str:
    months, months_abbr, days, days_abbr = NAMES.get(locale.split("-")[0].lower(), NAMES["en"])
    h12 = d.hour % 12 or 12
    table = {
        "%Y": f"{d.year:04d}", "%y": f"{d.year % 100:02d}", "%m": f"{d.month:02d}",
        "%-m": str(d.month), "%d": f"{d.day:02d}", "%-d": str(d.day),
        "%B": months[d.month - 1], "%b": months_abbr[d.month - 1],
        "%A": days[d.weekday()], "%a": days_abbr[d.weekday()],
        "%H": f"{d.hour:02d}", "%-H": str(d.hour), "%I": f"{h12:02d}", "%-I": str(h12),
        "%M": f"{d.minute:02d}", "%S": f"{d.second:02d}", "%p": "AM" if d.hour < 12 else "PM",
        "%j": f"{d.timetuple().tm_yday:03d}", "%%": "%",
    }
    return "".join(table.get(p, p) for p in parts)


# ---------------------------------------------------------------- rendering

class Ctx:
    def __init__(self, raw: dict):
        self.now = dt.datetime.fromisoformat(raw.get("now", "2026-03-18T14:05:09"))
        self.locale = raw.get("locale", "en")
        self.date_format = raw.get("date_format", "%Y-%m-%d")
        self.time_format = raw.get("time_format", "%H:%M")
        self.clipboard = raw.get("clipboard")
        self.snippets = raw.get("snippets", {})
        self.values = raw.get("values", {})


def expand(tree, ctx: Ctx, chain: tuple):
    """Replace snippet nodes with their parsed bodies (recursively)."""
    out = []
    for node in tree:
        if isinstance(node, str):
            out.append(node)
        elif node["kind"] == "snippet":
            abbr = node["abbr"]
            if abbr not in ctx.snippets:
                raise TemplateError("E_SNIPPET_NOT_FOUND")
            if abbr in chain:
                raise TemplateError("E_SNIPPET_CYCLE")
            if len(chain) >= MAX_DEPTH:
                raise TemplateError("E_TOO_DEEP")
            out.extend(expand(parse(ctx.snippets[abbr]), ctx, chain + (abbr,)))
        elif node["kind"] == "if":
            node = dict(node)
            node["then"] = expand(node["then"], ctx, chain)
            node["otherwise"] = expand(node["otherwise"], ctx, chain)
            out.append(node)
        else:
            out.append(node)
    return out


def walk(tree):
    for node in tree:
        if isinstance(node, dict):
            yield node
            if node["kind"] == "if":
                yield from walk(node["then"])
                yield from walk(node["otherwise"])


def collect_form(tree):
    """Fields in order of first appearance; explicit definitions override implicit toggles."""
    order, defs = [], {}
    for node in walk(tree):
        if node["kind"] == "field":
            name = node["name"]
            spec = {"name": name, "type": node["type"], "default": node["default"]}
            if node["type"] == "choice":
                spec["options"] = node["options"]
            if name not in defs:
                order.append(name)
                defs[name] = spec
            elif defs[name].get("implicit"):
                defs[name] = spec
            elif (defs[name]["type"], defs[name].get("options")) != (spec["type"],
                                                                       spec.get("options")):
                raise TemplateError("E_FIELD_CONFLICT")
        elif node["kind"] == "if" and node["field"] not in defs:
            order.append(node["field"])
            defs[node["field"]] = {"name": node["field"], "type": "toggle", "default": "no",
                                   "implicit": True, "bare": node["op"] == "truthy"}
        elif node["kind"] == "if" and defs[node["field"]].get("implicit"):
            defs[node["field"]]["bare"] &= node["op"] == "truthy"
    for spec in defs.values():
        if spec.get("implicit") and not spec["bare"]:
            raise TemplateError("E_FIELD_UNDEFINED")
    return [{k: v for k, v in defs[n].items() if k not in ("implicit", "bare")} for n in order]


def render(src: str, ctx: Ctx):
    tree = expand(parse(src), ctx, ())
    form = collect_form(tree)
    specs = {f["name"]: f for f in form}
    values = {n: normalize_newlines(ctx.values.get(n, s["default"])) for n, s in specs.items()}
    actions: list = []
    state = {"cursor_at": None, "wait": 0, "chars": 0}

    def emit_text(s: str):
        if not s:
            return
        state["chars"] += len(s)
        if state["chars"] > MAX_OUTPUT:
            raise TemplateError("E_OUTPUT_TOO_LARGE")
        if actions and "text" in actions[-1]:
            actions[-1]["text"] += s
        else:
            actions.append({"text": s})

    def run(nodes):
        for node in nodes:
            if isinstance(node, str):
                emit_text(node)
                continue
            kind = node["kind"]
            if kind in ("date", "time"):
                fmt = node["fmt"] or parse_format(
                    ctx.date_format if kind == "date" else ctx.time_format)
                emit_text(format_date(apply_shift(ctx.now, node["shift"]), fmt, ctx.locale))
            elif kind == "clipboard":
                clip = normalize_newlines(ctx.clipboard or "")
                emit_text(clip.strip() if node["trim"] else clip)
            elif kind == "cursor":
                if state["cursor_at"] is None:
                    state["cursor_at"] = state["chars"]
            elif kind == "field":
                v = values[node["name"]]
                if node["type"] == "toggle":
                    v = node["on"] if v == "yes" else node["off"]
                emit_text(v)
            elif kind == "key":
                if state["cursor_at"] is not None:
                    raise TemplateError("E_CURSOR_BEFORE_KEY")
                actions.append({"key": node["name"], "shift": node["shift"],
                                "times": node["times"]})
            elif kind == "wait":
                state["wait"] += node["ms"]
                if state["wait"] > MAX_WAIT_TOTAL:
                    raise TemplateError("E_WAIT_TOTAL")
                actions.append({"wait": node["ms"]})
            elif kind == "if":
                v, spec = values[node["field"]], specs[node["field"]]
                if node["op"] == "truthy":
                    ok = v == "yes" if spec["type"] == "toggle" else v != ""
                else:
                    ok = (v == node["value"]) == (node["op"] == "is")
                run(node["then"] if ok else node["otherwise"])

    run(tree)
    result = {"actions": actions, "form": form}
    if state["cursor_at"] is not None:
        tail = "".join(a["text"] for a in actions if "text" in a)[state["cursor_at"]:]
        result["cursor_back"] = count_graphemes(tail)
    return result


def count_graphemes(s: str) -> int:
    """Enough of UAX #29 for the examples: regional-indicator pairs and combining marks."""
    import unicodedata
    n, i = 0, 0
    while i < len(s):
        ri = 0x1F1E6 <= ord(s[i]) <= 0x1F1FF
        if ri and i + 1 < len(s) and 0x1F1E6 <= ord(s[i + 1]) <= 0x1F1FF:
            i += 1
        i += 1
        while i < len(s) and unicodedata.combining(s[i]):
            i += 1
        n += 1
    return n


# ---------------------------------------------------------------- checking examples

def check(directory: pathlib.Path, min_per_macro: int) -> int:
    failures, seen, per_macro, per_macro_err = [], set(), {}, {}
    for path in sorted(directory.glob("*.toml")):
        for ex in tomllib.loads(path.read_text(encoding="utf-8")).get("example", []):
            eid, group, exp = ex["id"], ex["macro"], ex["expect"]
            if eid in seen:
                failures.append(f"{eid}: duplicate id")
            seen.add(eid)
            if group not in MACRO_GROUPS:
                failures.append(f"{eid}: unknown macro group {group}")
            per_macro[group] = per_macro.get(group, 0) + 1
            try:
                got = render(ex["template"], Ctx(ex.get("ctx", {})))
            except TemplateError as e:
                got = {"error": e.code}
            if "error" in exp:
                per_macro_err[group] = per_macro_err.get(group, 0) + 1
                want = {"error": exp["error"]}
            else:
                actions = exp.get("actions")
                if actions is None:
                    actions = [{"text": exp["text"]}] if exp.get("text") else []
                actions = [({"shift": False, "times": 1} | a) if "key" in a else a
                           for a in actions]
                want = {"actions": actions, "form": exp.get("form", [])}
                if "cursor_back" in exp:
                    want["cursor_back"] = exp["cursor_back"]
            if got != want:
                failures.append(f"{eid} ({path.name}):\n  want {want}\n  got  {got}")
    for group in MACRO_GROUPS:
        if per_macro.get(group, 0) < min_per_macro:
            failures.append(f"group {group}: {per_macro.get(group, 0)} examples, "
                            f"need {min_per_macro}")
        if per_macro_err.get(group, 0) < 1:
            failures.append(f"group {group}: no error example")
    total = len(seen)
    if total < 80:
        failures.append(f"only {total} examples, need 80")
    for f in failures:
        print("FAIL", f)
    summary = ", ".join(f"{g} {per_macro.get(g, 0)}/{per_macro_err.get(g, 0)}"
                        for g in MACRO_GROUPS)
    print(f"{total} examples; per group total/errors: {summary}; {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    args = sys.argv[1:]
    minimum = 5
    if args[:1] == ["--min-per-macro"]:
        minimum, args = int(args[1]), args[2:]
    target = pathlib.Path(args[0]) if args else pathlib.Path(__file__).parent / "examples"
    sys.exit(check(target, minimum))
