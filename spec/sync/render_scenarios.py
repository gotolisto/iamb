#!/usr/bin/env python3
"""Writes the sync conflict scenarios out in words.

Reads `spec/sync/scenarios.toml` and replaces the part of `docs/sync-design.md` between the
two marker comments with one section per scenario: the starting state, the steps and the
defined outcome. The file is the source; the document is rendered from it, so the two cannot
drift apart.

    python3 spec/sync/render_scenarios.py          # rewrite the section
    python3 spec/sync/render_scenarios.py --check  # exit 1 when the document is out of date

The structure of the file is checked by `cargo test -p iamb-testkit --test sync_scenarios`,
which also checks that the document carries every scenario's heading and outcome.
"""

import sys
import textwrap
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCENARIOS = ROOT / "spec" / "sync" / "scenarios.toml"
DOCUMENT = ROOT / "docs" / "sync-design.md"
BEGIN = "<!-- scenarios:begin (rendered by spec/sync/render_scenarios.py; do not edit) -->"
END = "<!-- scenarios:end -->"
WIDTH = 100

REFS = ["profile", "parent", "folder", "snippet", "tag", "attachment", "group", "user"]

def wrap(text: str, indent: str = "") -> str:
    """One paragraph wrapped to the document's width."""
    return textwrap.fill(
        " ".join(text.split()),
        width=WIDTH,
        initial_indent=indent,
        subsequent_indent=indent,
        break_long_words=False,
        break_on_hyphens=False,
    )


def value(v) -> str:
    """A field value as a code span, so that Markdown inside it is not rendered."""
    if isinstance(v, bool):
        return "`true`" if v else "`false`"
    return f"`{v}`"


def fields(table: dict) -> str:
    return ", ".join(f"{k} {value(v)}" for k, v in table.items())


def refs(row: dict) -> str:
    return ", ".join(f"{k} `{row[k]}`" for k in REFS if k in row)


def offset(ms: int) -> str:
    hours = abs(ms) / 3_600_000
    amount = f"{hours / 24:g} days" if hours >= 48 else f"{hours:g} hours"
    return f"{amount} {'ahead of' if ms > 0 else 'behind'} true time"


def step_text(step: dict) -> str:
    at, do = step["at"], step["do"]
    row = f"`{step['row']}`" if "row" in step else ""
    simple = {
        "offline": f"{at} goes offline.",
        "online": f"{at} comes back online and syncs.",
        "manual": f"{at} stays connected but syncs only when a step says so.",
        "auto": f"{at} syncs by itself again.",
        "pull": f"{at} pulls.",
        "deliver": f"The held answer reaches {at}.",
        "channel_down": f"{at}'s live channel drops.",
        "channel_up": f"{at}'s live channel is back.",
        "delete": f"{at} deletes {row}.",
        "undelete": f"{at} restores the deleted {row}.",
        "decline": f"{at}: the user declines the invitation {row}.",
        "withdraw": f"{at}: the user withdraws the invitation {row}.",
        "remove_member": f"{at}: the user removes the member {row}.",
        "leave": f"{at}: the user leaves the group (membership {row}).",
        "sign_out": f"{at}: the user signs out.",
        "backup": "A backup of the server's database is taken.",
    }
    if do in simple:
        return simple[do]
    if do == "push":
        twice = " The request reaches the server twice." if step.get("fault") == "duplicate" else ""
        return f"{at} pushes.{twice}"
    if do == "clock":
        return f"{at}'s clock is {offset(step['offset_ms'])}."
    if do == "create":
        entity = step["entity"].replace("_", " ")
        where = f" ({refs(step)})" if refs(step) else ""
        what = f": {fields(step['fields'])}" if step.get("fields") else ""
        return f"{at} creates the {entity} {row}{where}{what}."
    if do == "edit":
        return f"{at} edits {row}: {fields(step['fields'])}."
    if do == "move":
        to = "the top level" if step["to"] == "top" else f"`{step['to']}`"
        return f"{at} moves {row} to {to}."
    if do == "invite":
        return (
            f"{at}: the user invites {step['user']} to `{step['group']}` as {step['role']} "
            f"(invitation `{step['creates']}`)."
        )
    if do == "accept":
        return f"{at}: the user accepts the invitation {row} (membership `{step['creates']}`)."
    if do == "set_role":
        return f"{at}: the user makes {row} a {step['role']}."
    if do == "transfer_ownership":
        return f"{at}: the user transfers the ownership of `{step['group']}` to {step['user']}."
    if do == "delete_group":
        return f"{at}: the user deletes the group `{step['group']}`."
    if do == "resolve":
        choice = "restores the other version" if step["choice"] == "restore" else "keeps the row as it is"
        return f"{at}: the user {choice} in the conflict on {row}."
    if do == "sign_in":
        return f"{at}: {step['user']} signs in."
    if do == "revoke_device":
        return f"{at}: the user revokes the device {step['device']}."
    if do == "upgrade":
        return f"{at} is updated to a build that speaks protocol {step['protocol']}."
    if do == "advance":
        return f"{step['days']} days pass."
    if do == "restore_backup":
        return f"The server's database is restored from the backup `{step['to']}`."
    if do == "set_protocol":
        return f"The server accepts protocol versions {step['min']} to {step['max']} from now on."
    if do == "set_limit":
        return f"The plan's limit `{step['name']}` is {step['value']}."
    raise ValueError(f"no wording for the step {do!r}")


def step_lines(number: int, step: dict) -> list[str]:
    label = f"`{step['id']}` · " if "id" in step else ""
    text = label + step_text(step)
    answers = []
    if "answer" in step:
        answers.append(f"`{step['answer']}`")
    answers += [f"`{row}` `{answer}`" for row, answer in step.get("answers", {}).items()]
    if answers:
        text += " Answer: " + ", ".join(answers) + "."
    if "note" in step:
        text += " *" + " ".join(step["note"].split()) + "*"
    prefix = f"{number}. "
    wrapped = textwrap.fill(
        text,
        width=WIDTH,
        initial_indent=prefix,
        subsequent_indent=" " * len(prefix),
        break_long_words=False,
        break_on_hyphens=False,
    )
    return wrapped.split("\n")


def start_lines(start: dict) -> list[str]:
    devices = []
    for device in start["devices"]:
        extra = []
        if not device.get("signed_in", True):
            extra.append("not signed in")
        if "protocol" in device:
            extra.append(f"protocol {device['protocol']}")
        note = f", {', '.join(extra)}" if extra else ""
        devices.append(f"{device['id']} ({device['user']}{note})")
    profiles = []
    for profile in start["profiles"]:
        local = f", local to {profile['local_to']}" if "local_to" in profile else ""
        profiles.append(f"`{profile['id']}` ({profile['kind']}, {profile['owner']}{local})")
    lines = [wrap("Devices: " + ", ".join(devices) + ". Profiles: " + ", ".join(profiles) + ".")]
    lines += ["", "| Row | Kind | Refers to | Rev | Fields |", "| --- | --- | --- | --- | --- |"]
    for row in start.get("rows", []):
        extra = []
        if row.get("deleted"):
            extra.append("deleted")
        if "by" in row:
            extra.append(f"written by {row['by']}")
        if row.get("only_on"):
            extra.append("only on " + ", ".join(row["only_on"]))
        cell = fields(row.get("fields", {}))
        if extra:
            cell = (cell + "; " if cell else "") + "; ".join(extra)
        lines.append(
            f"| `{row['id']}` | {row['entity'].replace('_', ' ')} | {refs(row)} | {row['rev']} "
            f"| {cell} |"
        )
    return lines


def cell(row: dict) -> str:
    if row["state"] == "absent":
        return "absent"
    text = f"{row['state']} · {row['version']}"
    if row.get("fields"):
        text += f" ({fields(row['fields'])})"
    return text


def device_rows(outcome: dict, device: str) -> dict:
    """The rows of a device's outcome by row id, with `same_as` followed."""
    found = outcome["device"][device]
    base = found.get("same_as")
    if base is None:
        rows = {}
    elif base == "server":
        rows = {r["row"]: r for r in outcome["server"]["rows"]}
    else:
        rows = dict(device_rows(outcome, base))
    for row in found.get("rows", []):
        rows[row["row"]] = row
    return rows


def outcome_lines(scenario: dict) -> list[str]:
    outcome = scenario["outcome"]
    server = outcome["server"]
    devices = [d["id"] for d in scenario["start"]["devices"]]
    by_device = {d: device_rows(outcome, d) for d in devices}
    lines = ["| Row | Server | " + " | ".join(devices) + " |"]
    lines.append("| --- | --- | " + " | ".join("---" for _ in devices) + " |")
    for row in server["rows"]:
        cells = []
        for device in devices:
            mine = cell(by_device[device][row["row"]])
            cells.append("=" if mine == cell(row) else mine)
        lines.append(f"| `{row['row']}` | {cell(row)} | " + " | ".join(cells) + " |")
    lines.append("")
    history: dict[str, list[str]] = {}
    for version in server["history"]:
        history.setdefault(version["row"], []).append(version["version"])
    kept = "; ".join(f"`{row}` {', '.join(versions)}" for row, versions in history.items())
    lines.append(wrap("- History (replaced versions the server keeps): " + (kept or "none") + "."))
    conflicts = "; ".join(
        f"`{c['row']}` {c['kind']}, winner {c['winner']}, loser {c['loser']}, "
        f"shown to {' and '.join(c['parties'])}, {c['state']}"
        for c in server["conflicts"]
    )
    lines.append(wrap("- Conflicts: " + (conflicts or "none") + "."))
    for device in devices:
        lines.append(wrap(f"- {device}: " + device_facts(outcome["device"][device]) + "."))
    return lines


def device_facts(found: dict) -> str:
    facts = ["shown " + (", ".join(found["shown"]) if found["shown"] else "nothing")]
    if found.get("kept"):
        facts.append("kept for the user: " + ", ".join(found["kept"]))
    if found.get("blocked"):
        facts.append("blocked: " + ", ".join(found["blocked"]))
    if found.get("outbox"):
        facts.append(f"{found['outbox']} change waiting in the outbox")
    if found.get("expands"):
        facts.append(
            "expands " + ", ".join(f"`{abbr}` as `{row}`" for abbr, row in found["expands"].items())
        )
    if found.get("review"):
        facts.append("held for review: " + ", ".join(f"`{r}`" for r in found["review"]))
    if found.get("broken"):
        facts.append("reference broken: " + ", ".join(f"`{r}`" for r in found["broken"]))
    if found.get("access"):
        facts.append(
            "permission " + ", ".join(f"`{folder}` {p}" for folder, p in found["access"].items())
        )
    return "; ".join(facts)


def scenario_lines(scenario: dict) -> list[str]:
    lines = [f"#### {scenario['id']} · {scenario['title']}", ""]
    lines.append(
        wrap(
            f"Category: {scenario['category']}. Covers: "
            + ", ".join(scenario["covers"])
            + ". Rules: "
            + ", ".join(scenario["rules"])
            + "."
        )
    )
    lines += ["", wrap("**Start.** " + scenario["given"]), ""]
    lines += start_lines(scenario["start"])
    lines += ["", "**Steps.**", ""]
    for number, step in enumerate(scenario["step"], start=1):
        lines += step_lines(number, step)
    lines += ["", wrap("**Outcome.** " + scenario["outcome"]["summary"]), ""]
    lines += outcome_lines(scenario)
    lines.append("")
    return lines


def render(scenarios: list[dict]) -> str:
    lines = [BEGIN, "", "| Scenario | Title | Category | Covers |", "| --- | --- | --- | --- |"]
    for scenario in scenarios:
        lines.append(
            f"| {scenario['id']} | {scenario['title']} | {scenario['category']} "
            f"| {', '.join(scenario['covers'])} |"
        )
    lines.append("")
    for scenario in scenarios:
        lines += scenario_lines(scenario)
    lines.append(END)
    return "\n".join(lines)


def main() -> int:
    with SCENARIOS.open("rb") as handle:
        scenarios = tomllib.load(handle)["scenario"]
    section = render(scenarios)
    text = DOCUMENT.read_text(encoding="utf-8")
    try:
        before, rest = text.split(BEGIN, 1)
        _, after = rest.split(END, 1)
    except ValueError:
        print(f"{DOCUMENT}: the scenario markers are missing", file=sys.stderr)
        return 2
    updated = before + section + after
    if "--check" in sys.argv[1:]:
        if updated != text:
            print(f"{DOCUMENT} is out of date: run python3 spec/sync/render_scenarios.py", file=sys.stderr)
            return 1
        print(f"{len(scenarios)} scenarios, the document is up to date")
        return 0
    if updated != text:
        DOCUMENT.write_text(updated, encoding="utf-8")
    print(f"{len(scenarios)} scenarios rendered into {DOCUMENT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
