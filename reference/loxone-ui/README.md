# Loxone UI reference dumps

Read-only material from the Loxone media client. **Protocol truth** for this audioserver is in [`docs/`](../../docs/) and [`src/`](../../src/) — use these files when investigating UI behaviour or regenerating wire inventories.

The three JavaScript bundles are Loxone's own code and are **not in this repository**. To use them, copy them from the Loxone app (Web UI or desktop app resources) into this folder; they are git-ignored. The two traces are tracked, with addresses, serials and login data replaced.

| File | Provenance | Use |
|------|------------|-----|
| `scripts6.js` (local only) | Loxone media bundle (`MediaEnum`, wire path templates) | `just wire-docs` / [`tools/extract_ui_wire_commands.py`](../../tools/extract_ui_wire_commands.py) |
| `controls1.js` (local only) | Zone control widgets (`sendAudioZoneCommand`) | Legacy transport UI |
| `comps.js` (local only) | React media schemas (Zod), miniplayer, loudness/EQ | `cmd` discriminated unions, scrubber behaviour |
| `trace.txt` | Captured 7091 session | Bootstrap / browse reference |
| `trace-7095.txt` | Captured 7095 session | Pairing / config reference |

These files are large and minified; read the docs and code first.
