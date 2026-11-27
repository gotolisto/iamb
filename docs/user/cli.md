# Command line

The `iamb` command manages your library from a terminal. It works on macOS, Windows and Linux.
Use it to add many snippets at once, export a backup, or check what a snippet would type.

Every command below is a real command of the tool. The examples were run against a test library.

## Run it

```text
iamb <command> [options]
```

`iamb --help` lists the commands. `iamb <command> --help` shows the options of one command.

The tool uses the library through Iamb's background service when the service is running. When it
is not, the tool opens the library file itself. If the service is starting up, the tool says
**the service is starting, try again**.

### Where the library is

By default, the tool uses your library folder for each system: `~/Library/Application Support/Iamb`
on macOS, `%APPDATA%\Iamb` on Windows, and `$XDG_DATA_HOME/iamb` (else `~/.local/share/iamb`) on
Linux. To use another folder, pass
`--data-dir`, or set the `IAMB_DATA_DIR` environment variable. The flag wins over the variable.

```text
iamb --data-dir ~/Iamb-test list
```

## Add a snippet

```text
iamb add ";sig" "Best regards, Ada" --label Signature --folder "Work / Email"
```

The command prints the new snippet's id.

| Option | What it does |
| --- | --- |
| `--body-file <PATH>` | Read the text from a file. Use `-` to read from standard input. Use this instead of the text. |
| `--label <TEXT>` | A name shown in lists and search. |
| `--folder <PATH>` | Folder names separated by ` / `. Missing folders are created. |
| `--trigger immediate` or `delimiter` | When the snippet expands. Default: after a space or punctuation. |
| `--case exact`, `insensitive` or `adaptive` | How capital letters are matched. Default: `insensitive`. |
| `--disabled` | Create the snippet turned off. |

Body text can use the [template](templates.md) macros. Quote it so your shell does not change it:

```text
iamb add ";hi" "Hi {field name=\"Name\"}, {cursor}"
```

## List snippets

```text
iamb list
iamb list --folder Work --json
```

`list` prints a table, sorted by abbreviation. `--folder` shows that folder and the folders below
it. `--json` prints JSON instead of a table.

## Remove a snippet

```text
iamb remove ";sig"
```

Removes the snippet with that abbreviation. The abbreviation is not case-sensitive. The removal is
a soft delete: the snippet is marked deleted, not erased from the file at once.

## Import and export

```text
iamb export library.json
iamb export library.csv --format csv
iamb import library.json --on-conflict skip
```

See [Import and export](import-export.md) for the file formats and what happens when an abbreviation
is already in use.

## Check what a snippet would type

```text
iamb expand ";sig "
iamb expand ";hi " --field Name=Ada --show-cursor
```

`expand` runs the matching snippet without typing anything, and prints what it would type. Nothing
changes in your library. Use it to test a template before you rely on it.

| Option | What it does |
| --- | --- |
| `--field NAME=VALUE` | A fill-in value. Repeat for each field. Fields you leave out use their defaults. |
| `--now YYYY-MM-DDTHH:MM:SS` | The clock to use, so dates are predictable. Default: the current time in UTC. |
| `--clipboard <TEXT>` | The clipboard text for `{clipboard}`. Default: empty. |
| `--show-cursor` | Show where the caret would be, as `|`. |
| `--actions` | Print each step (type text, delete, show a form) on its own line. |

The examples above print:

```text
Best regards, Ada 
Hi Ada, |
```

Exit status: `0` when a snippet expanded, `3` when nothing expanded, `1` on an error. A template with an
error also gives `3`: the tool prints the error code and the typed text unchanged.

## Exit status

| Status | Meaning |
| --- | --- |
| `0` | The command worked. For `expand`, a snippet expanded. |
| `1` | An error. The message says what went wrong. |
| `2` | The command line itself is wrong: an unknown command or option, or a missing text. The tool prints a usage message. |
| `3` | `expand` only: nothing expanded, because no snippet matched or the template has an error. |

## Related

- [Import and export](import-export.md)
- [Templates](templates.md)
