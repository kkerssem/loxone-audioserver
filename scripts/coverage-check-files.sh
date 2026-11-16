#!/usr/bin/env bash
# Per-file Tier A/B coverage floors + overall in-scope gate. Requires lcov.info from cargo llvm-cov.
set -euo pipefail

LCOV="${1:-target/llvm-cov/lcov.info}"
if [[ ! -f "$LCOV" ]]; then
  echo "coverage-check-files: missing $LCOV" >&2
  exit 1
fi

python3 - "$LCOV" <<'PY'
from __future__ import annotations

import sys
from pathlib import PurePath

path = sys.argv[1]
files: dict[str, tuple[int, int]] = {}
current: str | None = None
lf = lh = 0

with open(path, encoding="utf-8") as f:
    for raw in f:
        line = raw.strip()
        if line.startswith("SF:"):
            if current is not None and lf:
                files[current] = (lh, lf)
            current = line[3:]
            lf = lh = 0
        elif line == "end_of_record":
            if current is not None and lf:
                files[current] = (lh, lf)
            current = None
            lf = lh = 0
        elif current is not None and line.startswith("LF:"):
            lf = int(line[3:])
        elif current is not None and line.startswith("LH:"):
            lh = int(line[3:])

line_totals = {"covered": 0, "count": 0}
pcts: dict[str, float] = {}
uncovered_rows: list[tuple[int, float, str]] = []

for name, (covered, count) in files.items():
    pct = 100.0 * covered / count if count else 0.0
    pcts[name] = pct
    line_totals["covered"] += covered
    line_totals["count"] += count
    unc = count - covered
    if unc > 0:
        short = name.split("loxone-audioserver/")[-1] if "loxone-audioserver/" in name else name
        uncovered_rows.append((unc, pct, short))

overall = (
    100.0 * line_totals["covered"] / line_totals["count"]
    if line_totals["count"]
    else 0.0
)
print(
    f"In-scope overall line coverage: {overall:.1f}% "
    f"({line_totals['covered']}/{line_totals['count']} lines, {len(pcts)} files)"
)

OVERALL_FLOOR = 80.0
hard_failures = []
if overall < OVERALL_FLOOR:
    hard_failures.append(
        f"Overall in-scope: {overall:.1f}% (need ≥{OVERALL_FLOOR:.0f}%)"
    )

uncovered_rows.sort(reverse=True)
if uncovered_rows:
    print("Top uncovered files:")
    for unc, pct, short in uncovered_rows[:10]:
        print(f"  {unc:4d} lines ({pct:5.1f}%)  {short}")


def match_any(name: str, patterns: list[str]) -> bool:
    p = PurePath(name)
    n = name.replace("\\", "/")
    for pat in patterns:
        if pat.endswith("/"):
            if pat.rstrip("/") in p.parts:
                return True
        elif name.endswith(pat) or pat in n:
            return True
    return False

tier_a = [
    "zone_registry.rs",
    "zone_publish.rs",
    "zone_enrich.rs",
    "wire_response.rs",
    "player_state.rs",
    "recents.rs",
    "radio/state.rs",
    "radio/search_cache.rs",
    "config/mod.rs",
    "config/loxone.rs",
    "config/zone.rs",
    "config/crc.rs",
    "speaker/map.rs",
    "speaker/binding.rs",
]

tier_b = [
    "coordinator/mod.rs",
    "connect_bootstrap.rs",
    "zone_transport.rs",
    "remote/router.rs",
    "miniserver/router.rs",
    "cast/sync.rs",
    "speaker/manager.rs",
    "coordinator/speaker.rs",
]

failures = []
tier_b_pcts = []
for name, pct in sorted(pcts.items()):
    if match_any(name, tier_a):
        if pct < 90.0:
            failures.append(f"Tier A {name}: {pct:.1f}% (need ≥90%)")
    elif match_any(name, tier_b):
        tier_b_pcts.append(pct)
        if pct < 80.0:
            failures.append(f"Tier B {name}: {pct:.1f}% (need ≥80%)")

if tier_b_pcts:
    avg_b = sum(tier_b_pcts) / len(tier_b_pcts)
    print(f"Tier B average line coverage: {avg_b:.1f}% ({len(tier_b_pcts)} files)")
    if avg_b < 80.0:
        failures.append(f"Tier B average: {avg_b:.1f}% (need ≥80%)")

hard_failures.extend(f for f in failures if f.startswith("Tier A"))
if hard_failures:
    print("Coverage gate failures:", file=sys.stderr)
    for line in hard_failures:
        print(f"  {line}", file=sys.stderr)
    sys.exit(1)

soft_failures = [f for f in failures if f.startswith("Tier B")]
if soft_failures:
    print("Tier B per-file below 80% (average gate passed):", file=sys.stderr)
    for line in soft_failures:
        print(f"  {line}", file=sys.stderr)

print("Coverage gates OK (overall ≥80%, Tier A ≥90%)")
PY
