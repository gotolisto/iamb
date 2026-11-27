# Import and export

You can copy your library to a file and load it back. Use this for backups, for moving to another
computer, or for editing many snippets in a spreadsheet.

The import and export commands are part of the [command line](cli.md) tool. The desktop app has no
import or export window yet.

<!-- TODO: the paste-fidelity notes refer to an Import button in the library window. The desktop app does not have one yet. -->

## Export

```text
iamb export library.json
iamb export library.csv
```

The format comes from the file name: `.json` or `.csv`. To choose it yourself, pass
`--format json` or `--format csv`. A file name with another extension needs `--format`.

The export writes your snippets, their folders and their tags.

## Import

```text
iamb import library.json
iamb import library.csv --on-conflict skip
```

Import reads the whole file and checks it first. If something is wrong, nothing is written.

### When an abbreviation is already in use

Each abbreviation can belong to one snippet, and capitals do not count (`;Sig` is `;sig`). What
happens when a file has one you already use depends on `--on-conflict`:

| Choice | What it does |
| --- | --- |
| `skip` (the default) | Keeps your snippet and skips the one in the file. The result says how many were skipped. |
| `fail` | Stops and writes nothing. |

## JSON files

JSON is the complete format. It keeps everything, including the images of formatted snippets.

The file has a header with `"format": "iamb-library"` and `"version": 1`. It holds folders, snippets,
tags and how they link. Images are stored inside the file, so the file can be large.

## CSV files

CSV is one row per snippet. It opens in a spreadsheet and is easy to edit by hand. It does not hold
folders or tags on their own. Folder names and tag names are written as text in their columns. It does
not hold images.

The columns, in order:

| Column | What goes in it |
| --- | --- |
| `abbreviation` | The abbreviation, for example `;sig`. |
| `label` | The name shown in lists. Optional. |
| `body` | The text, with any [template](templates.md) macros. |
| `folder` | The folder path, with names separated by ` / `. Empty means the folder **Imported**. |
| `tags` | Tag names separated by `;`. |
| `trigger` | `immediate` or `delimiter`. |
| `case_mode` | `exact`, `insensitive` or `adaptive`. |
| `word_start` | `true` or `false`. |
| `enabled` | `true` or `false`. |
| `kind` | `plain` or `rich`. |

If a folder name contains ` / `, a tag name contains `;`, or a name contains a backslash or starts or
ends with a space, the export writes a backslash in front of that character, so the name reads back
the same. A backslash in any
other place is read as an ordinary character, so Windows paths work as they are.

## Related

- [Command line](cli.md)
- [Snippets](snippets.md)
