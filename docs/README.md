# Documentation index

Read order for contributors and coding agents:

1. **[architecture.md](architecture.md)** — Runtime topology, modules, `IoCommand` bus, persistence
2. **[traffic-paths.md](traffic-paths.md)** — Which commands use port 7091 vs 7095; audit capture
3. **[web-ui-protocol.md](web-ui-protocol.md)** — 7091 bootstrap, browse, transport
4. **[miniserver-protocol.md](miniserver-protocol.md)** — 7095 pairing, `setconfig`, secure session
5. **[radio-cast.md](radio-cast.md)** — TuneIn / custom radio over Google Cast

## Deep dives

| Document | Topic |
|----------|--------|
| [ui-wire-commands.md](ui-wire-commands.md) | Searchable wire command inventory (from `scripts6.js`) |
| [ui-wire-commands.json](ui-wire-commands.json) | Machine-readable wire index |

## Agent entry

- [AGENTS.md](../AGENTS.md) — build/test commands, module map, constraints
- [CONTRIBUTING.md](../CONTRIBUTING.md) — PR checklist, validation recipes

## Loxone UI reference (not protocol truth)

Minified client bundles and session captures: [../reference/loxone-ui/README.md](../reference/loxone-ui/README.md).
