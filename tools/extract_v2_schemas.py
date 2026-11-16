#!/usr/bin/env python3
"""Extract V2 schema/repository name hints from reference/loxone-ui/comps.js."""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COMPS = ROOT / "reference" / "loxone-ui" / "comps.js"
OUT = ROOT / "docs" / "v2-schema-inventory.json"

SCHEMA_RE = re.compile(r"\b([A-Z][A-Za-z0-9]*Schema)\b")
REPO_RE = re.compile(r"\b([A-Z][A-Za-z0-9]*Repository)\b")
SCHEME_RE = re.compile(r"\b([A-Z][A-Za-z0-9]*Scheme)\b")
WIRE_RE = re.compile(r'"(audio(?:/cfg)?/[^"]+)"')


def main() -> None:
    if not COMPS.exists():
        print(f"comps.js not found at {COMPS} — skip inventory")
        return

    text = COMPS.read_text(encoding="utf-8", errors="replace")
    schemas = sorted(set(SCHEMA_RE.findall(text)) | set(SCHEME_RE.findall(text)))
    repos = sorted(set(REPO_RE.findall(text)))
    wire_literals = sorted(set(WIRE_RE.findall(text)))

    payload = {
        "generated_from": {"comps.js": str(COMPS.relative_to(ROOT)), "date": date.today().isoformat()},
        "stats": {
            "schemas_and_schemes": len(schemas),
            "repositories": len(repos),
            "wire_literals": len(wire_literals),
        },
        "schemas_and_schemes": schemas,
        "repositories": repos,
        "wire_literals": wire_literals[:200],
    }
    OUT.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload["stats"], indent=2))


if __name__ == "__main__":
    main()
