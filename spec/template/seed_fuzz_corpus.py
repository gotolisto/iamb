#!/usr/bin/env python3
"""Write each distinct template from spec/template/examples/*.toml to its own file.

Used to seed the fuzz corpus: python3 spec/template/seed_fuzz_corpus.py fuzz/corpus/template
"""

import hashlib
import pathlib
import sys
import tomllib

out = pathlib.Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
seen = set()
for path in sorted((pathlib.Path(__file__).parent / "examples").glob("*.toml")):
    for ex in tomllib.loads(path.read_text(encoding="utf-8")).get("example", []):
        data = ex["template"].encode("utf-8")
        if data in seen:
            continue
        seen.add(data)
        (out / hashlib.sha256(data).hexdigest()[:16]).write_bytes(data)
print(f"wrote {len(seen)} seed inputs to {out}")
