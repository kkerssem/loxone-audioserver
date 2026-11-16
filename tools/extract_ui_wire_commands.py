#!/usr/bin/env python3
"""Extract Loxone UI wire commands from scripts6.js into docs/ui-wire-commands.*"""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS6 = ROOT / "reference" / "loxone-ui" / "scripts6.js"
REMOTE_RS = ROOT / "src" / "remote" / "mod.rs"
AUDIT_LOG = ROOT / "command-audit.log"
OUT_JSON = ROOT / "docs" / "ui-wire-commands.json"
OUT_MD = ROOT / "docs" / "ui-wire-commands.md"

BROWSE_KEYWORDS = (
    "getmediafolder",
    "getservicefolder",
    "globalsearch",
    "getplaylists",
    "getservices",
    "getavailableservices",
    "getradios",
    "serviceplay",
    "library/play",
    "getqueue",
    "recent",
    "getroomfavs",
    "search",
    "playurl",
    "addurl",
    "favoriteplay",
    "playlist/play",
)

VALID_PATH = re.compile(r"^audio(?:/cfg)?/[a-zA-Z0-9_./%-]+$")


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def extract_balanced_block(text: str, marker: str) -> str:
    idx = text.find(marker)
    if idx < 0:
        return ""
    # Prefer the opening brace immediately after `Name:` (e.g. Commands:{...).
    after = idx + len(marker)
    if after < len(text) and text[after] == "{":
        brace = after
    else:
        brace = text.find("{", after)
        if brace < 0:
            return ""
    depth = 0
    for i in range(brace, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return text[brace + 1 : i]
    return ""


def js_object_to_dict(block: str) -> dict:
    """Best-effort parse of a minified JS object literal (string values only)."""

    def parse_object(s: str, pos: int = 0) -> tuple[dict, int]:
        out: dict = {}
        while pos < len(s):
            while pos < len(s) and s[pos] in ", \t\n\r":
                pos += 1
            if pos >= len(s) or s[pos] == "}":
                return out, pos + 1
            km = re.match(r"([A-Za-z_][A-Za-z0-9_]*):", s[pos:])
            if not km:
                break
            key = km.group(1)
            pos += km.end()
            if s[pos] == "{":
                nested, pos = parse_object(s, pos + 1)
                out[key] = nested
            elif s[pos] == '"':
                end = pos + 1
                while end < len(s):
                    if s[end] == "\\":
                        end += 2
                        continue
                    if s[end] == '"':
                        break
                    end += 1
                out[key] = json.loads(s[pos : end + 1])
                pos = end + 1
            else:
                break
        return out, pos

    parsed, _ = parse_object(block)
    return parsed


def flatten_strings(prefix: str, value: object, out: list[dict]) -> list[dict]:
    if isinstance(value, dict):
        for k, v in value.items():
            flatten_strings(f"{prefix}.{k}" if prefix else k, v, out)
    elif isinstance(value, str):
        out.append({"enum_path": prefix, "suffix_or_path": value})
    return out


def normalize_template(path: str) -> str:
    p = path
    p = re.sub(r"%s", "{param}", p)
    p = re.sub(r"%i", "{param}", p)
    p = re.sub(r"/\d+(?=/|$)", "/{n}", p)
    return p


def extract_cfg_paths(text: str) -> list[str]:
    paths = set()
    for m in re.finditer(r'"((?:audio(?:/cfg)?/)[^"]{1,120})"', text):
        s = m.group(1)
        if VALID_PATH.match(s) and s not in ("audio/", "audio/cfg/"):
            paths.add(s)
    return sorted(paths)


def extract_rust_routes(text: str) -> list[str]:
    return sorted(
        set(re.findall(r'\.insert\(\s*"([^"]+)"', text, flags=re.MULTILINE))
    )


def extract_audit_commands(text: str) -> list[str]:
    cmds = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) >= 2:
            cmds.append(parts[-1])
    return sorted(set(cmds))


def is_browse_related(path: str) -> bool:
    lower = path.lower()
    return any(k in lower for k in BROWSE_KEYWORDS)


def route_status(ui_path: str, rust_routes: list[str], audit_cmds: list[str]) -> dict:
    norm_ui = normalize_template(ui_path)

    def route_matches(r: str) -> bool:
        norm_r = normalize_template(r)
        if norm_r == norm_ui:
            return True
        # UI sprintf templates vs Rust matchit params (same segments, different placeholders).
        ui_parts = norm_ui.rstrip("/").split("/")
        rust_parts = norm_r.rstrip("/").split("/")
        if len(ui_parts) != len(rust_parts):
            return False
        for a, b in zip(ui_parts, rust_parts):
            if a == b:
                continue
            if a in ("{param}", "{n}") and b.startswith("{"):
                continue
            if b in ("{param}", "{n}") and a.startswith("{"):
                continue
            return False
        return True

    matched_rust = [r for r in rust_routes if route_matches(r)]
    observed = [c for c in audit_cmds if c == ui_path or c.startswith(ui_path.rstrip("/"))]
    if matched_rust:
        impl = "routed"
    elif observed:
        impl = "observed_not_routed"
    else:
        impl = "ui_only"
    return {
        "implementation": impl,
        "rust_routes": matched_rust,
        "observed_in_audit": observed[:5],
    }


def zone_command_rows(audio_commands: dict) -> list[dict]:
    rows = []
    for entry in flatten_strings("", audio_commands, []):
        suffix = entry["suffix_or_path"]
        if suffix.startswith("audio/"):
            wire = suffix
        else:
            wire = f"audio/{{playerid}}/{suffix}"
        rows.append(
            {
                "enum_path": f"MediaEnum.AudioCommands.{entry['enum_path']}",
                "wire_template": wire,
                "category": "transport"
                if entry["enum_path"]
                in {
                    "PLAY",
                    "PAUSE",
                    "STATUS",
                    "QUEUE.GET",
                    "VOLUME.SET_TO",
                }
                or entry["enum_path"].startswith("QUEUE.")
                or entry["enum_path"].startswith("VOLUME.")
                or entry["enum_path"].startswith("SEEK.")
                or entry["enum_path"].startswith("REPEAT.")
                or entry["enum_path"].startswith("SHUFFLE.")
                else "browse_or_play"
                if is_browse_related(wire)
                or entry["enum_path"].startswith(("LIBRARY.", "SERVICE.", "SEARCH.", "PLAYLIST.", "FAVORITE.", "ZONE_FAV."))
                else "other",
            }
        )
    return sorted(rows, key=lambda r: r["enum_path"])


def main() -> None:
    scripts6 = read_text(SCRIPTS6)
    remote_rs = read_text(REMOTE_RS)
    audit = read_text(AUDIT_LOG) if AUDIT_LOG.exists() else ""

    cfg_paths = extract_cfg_paths(scripts6)
    rust_routes = extract_rust_routes(remote_rs)
    audit_cmds = extract_audit_commands(audit)

    audio_commands = js_object_to_dict(extract_balanced_block(scripts6, "AudioCommands:"))
    commands_block = scripts6.find("Commands:{AUDIO_STATUS_ALL")
    commands = js_object_to_dict(
        extract_balanced_block(scripts6[commands_block:], "Commands:")
        if commands_block >= 0
        else ""
    )
    features = js_object_to_dict(extract_balanced_block(scripts6, "Features:"))

    cfg_commands = []
    for entry in flatten_strings("", commands, []):
        if entry["suffix_or_path"].startswith("audio/"):
            path = entry["suffix_or_path"]
            cfg_commands.append(
                {
                    "enum_path": f"MediaEnum.Commands.{entry['enum_path']}",
                    "wire_template": path,
                    **route_status(path, rust_routes, audit_cmds),
                    "browse_related": is_browse_related(path),
                }
            )

    cfg_path_rows = []
    for path in cfg_paths:
        cfg_path_rows.append(
            {
                "wire_template": path,
                "normalized": normalize_template(path),
                **route_status(path, rust_routes, audit_cmds),
                "browse_related": is_browse_related(path),
            }
        )

    payload = {
        "generated_from": {
            "scripts6.js": str(SCRIPTS6.relative_to(ROOT)),
            "src/remote/mod.rs": str(REMOTE_RS.relative_to(ROOT)),
            "command-audit.log": str(AUDIT_LOG.relative_to(ROOT)) if audit else None,
            "date": date.today().isoformat(),
        },
        "stats": {
            "cfg_path_literals": len(cfg_paths),
            "rust_routes": len(rust_routes),
            "audit_unique_commands": len(audit_cmds),
            "audio_command_enums": len(flatten_strings("", audio_commands, [])),
            "commands_cfg_enums": len(cfg_commands),
        },
        "implementation_legend": {
            "routed": "Handler registered in src/remote/mod.rs",
            "observed_not_routed": "Seen in command-audit.log but no matching router entry",
            "ui_only": "Present in scripts6.js only (not routed or observed yet)",
        },
        "features": [
            {
                "name": k,
                "api": v.get("api") if isinstance(v, dict) else None,
                "firmware": v.get("firmware") if isinstance(v, dict) else None,
            }
            for k, v in sorted(features.items())
        ],
        "cfg_paths": cfg_path_rows,
        "cfg_commands": sorted(cfg_commands, key=lambda r: r["wire_template"]),
        "zone_commands": zone_command_rows(audio_commands),
        "rust_routes": [{"route": r, "browse_related": is_browse_related(r)} for r in rust_routes],
        "audit_commands": [{"command": c, "browse_related": is_browse_related(c)} for c in audit_cmds],
    }

    OUT_JSON.parent.mkdir(parents=True, exist_ok=True)
    OUT_JSON.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    browse_paths = [r for r in cfg_path_rows if r["browse_related"]]
    browse_zone = [r for r in payload["zone_commands"] if r["category"] == "browse_or_play"]

    md = []
    md.append("# Loxone UI wire command appendix")
    md.append("")
    md.append(
        "Searchable index extracted from `scripts6.js` (`MediaEnum`), "
        "cross-referenced with [`src/remote/mod.rs`](../src/remote/mod.rs) routes and "
        "[`command-audit.log`](../command-audit.log)."
    )
    md.append("")
    md.append("Regenerate:")
    md.append("")
    md.append("```bash")
    md.append("python3 tools/extract_ui_wire_commands.py")
    md.append("```")
    md.append("")
    md.append(f"Machine-readable: [`ui-wire-commands.json`](ui-wire-commands.json) (generated {payload['generated_from']['date']}).")
    md.append("")
    md.append("## Implementation legend")
    md.append("")
    md.append("| Status | Meaning |")
    md.append("|--------|---------|")
    md.append("| `routed` | Handler in `src/remote/mod.rs` |")
    md.append("| `observed_not_routed` | In `command-audit.log` but no router match |")
    md.append("| `ui_only` | In UI bundle only |")
    md.append("")
    md.append("## Browse-related config paths (scripts6.js literals)")
    md.append("")
    md.append("| Wire template | Normalized | Status | Rust route(s) |")
    md.append("|---------------|------------|--------|---------------|")
    for row in browse_paths:
        routes = ", ".join(f"`{r}`" for r in row["rust_routes"]) or "—"
        md.append(
            f"| `{row['wire_template']}` | `{row['normalized']}` | {row['implementation']} | {routes} |"
        )
    md.append("")
    md.append("## All config path literals (`audio/cfg/...`)")
    md.append("")
    md.append("| Wire template | Browse | Status |")
    md.append("|---------------|--------|--------|")
    for row in cfg_path_rows:
        md.append(
            f"| `{row['wire_template']}` | {'yes' if row['browse_related'] else ''} | {row['implementation']} |"
        )
    md.append("")
    md.append("## `MediaEnum.Commands` config paths")
    md.append("")
    md.append("| Enum path | Wire template | Browse | Status |")
    md.append("|-----------|---------------|--------|--------|")
    for row in payload["cfg_commands"]:
        md.append(
            f"| `{row['enum_path']}` | `{row['wire_template']}` | {'yes' if row['browse_related'] else ''} | {row['implementation']} |"
        )
    md.append("")
    md.append("## Zone wire templates (`audio/{playerid}/…` suffixes from `MediaEnum.AudioCommands`)")
    md.append("")
    md.append("Built at runtime as `audio/` + `playerid` + `/` + suffix unless the value is already a full `audio/cfg/…` path.")
    md.append("")
    md.append("| Enum path | Wire template | Category |")
    md.append("|-----------|---------------|----------|")
    for row in browse_zone:
        md.append(f"| `{row['enum_path']}` | `{row['wire_template']}` | {row['category']} |")
    md.append("")
    md.append("### All zone command suffixes")
    md.append("")
    md.append("| Enum path | Wire template | Category |")
    md.append("|-----------|---------------|----------|")
    for row in payload["zone_commands"]:
        md.append(f"| `{row['enum_path']}` | `{row['wire_template']}` | {row['category']} |")
    md.append("")
    md.append("## Rust routes (`src/remote/mod.rs`)")
    md.append("")
    md.append("| Route | Browse |")
    md.append("|-------|--------|")
    for row in payload["rust_routes"]:
        md.append(f"| `{row['route']}` | {'yes' if row['browse_related'] else ''} |")
    md.append("")
    md.append("## Observed commands (`command-audit.log`)")
    md.append("")
    md.append("| Command | Browse |")
    md.append("|---------|--------|")
    for row in payload["audit_commands"]:
        if row["browse_related"] or row["command"].startswith("audio/"):
            md.append(f"| `{row['command']}` | {'yes' if row['browse_related'] else ''} |")
    md.append("")
    md.append("## Feature gates (`MediaEnum.Features`)")
    md.append("")
    md.append("Relevant entries for browse/search:")
    md.append("")
    md.append("| Feature | API | Firmware |")
    md.append("|---------|-----|----------|")
    for feat in payload["features"]:
        name = feat["name"]
        if any(k in name for k in ("SEARCH", "LIBRARY", "PLAYLIST", "SERVICE", "LOGIN", "MEDIA")):
            md.append(f"| `{name}` | {feat['api'] or '—'} | {feat['firmware'] or '—'} |")

    OUT_MD.write_text("\n".join(md) + "\n", encoding="utf-8")
    print(f"Wrote {OUT_JSON.relative_to(ROOT)}")
    print(f"Wrote {OUT_MD.relative_to(ROOT)}")
    print(json.dumps(payload["stats"], indent=2))


if __name__ == "__main__":
    main()
