# 7091 / 7095 wire handler overlap

Short map of commands that appear on both Loxone ports (7091 Web UI, 7095 Miniserver upstream) and whether behaviour or reply builders are shared.

See also [traffic-paths.md](traffic-paths.md) and [architecture.md](architecture.md).

## Shared infrastructure (already extracted)

| Concern | Implementation |
|---------|----------------|
| Zone transport (play/pause/volume/seek) | [`src/zone_transport.rs`](../src/zone_transport.rs) — both ports |
| `getqueue` | [`src/remote/router.rs`](../src/remote/router.rs) `getqueue_router` + [`try_dispatch_getqueue`](../src/remote/mod.rs); 7095 falls back when its router misses |
| Wire envelopes (`*_result` before `command`) | [`src/wire_response.rs`](../src/wire_response.rs) |
| `not_implemented` stub | [`src/loxone_wire.rs`](../src/loxone_wire.rs) `not_implemented_reply` |
| Base64 `{data}` decode + stripped command echo | [`src/loxone_wire.rs`](../src/loxone_wire.rs) — miniserver config handshake |

## Per-path overlap

| Path / concern | 7091 (`remote`) | 7095 (`miniserver`) | Share reply builder? |
|----------------|-----------------|---------------------|----------------------|
| `audio/cfg/diagnosis` | Empty stub | `not_implemented` | **No** — different behaviour |
| `audio/cfg/identify` | mac + services list | `audio/cfg/identify/off` → `off_result` | **No** — different routes and shapes |
| `audio/cfg/reboot` | `not_implemented` | `not_implemented` | **Yes** — `not_implemented_reply` |
| `audio/cfg/getroomfavs/...` | Paged empty favs (`handlers/bootstrap.rs`) | `getroomfavs/all` static stub | **Partial** — same empty list idea, different paths |
| `getqueue` | Primary UI path | Fallback when config router misses | **Already shared** |
| `playername`, `playeropts`, `volumes`, `groupopts`, `speakertype` | Not on 7091 router | Base64 `{data}` → `IoCommand` → reply | **Yes** — `loxone_wire` helpers |
| `getconfig`, `setconfig`, `ready`, secure pairing | Not on 7091 | 7095 handshake only | Miniserver-only |

## When adding a new wire command

1. Check this table and [ui-wire-commands.json](ui-wire-commands.json).
2. If both ports need the same path, prefer one route table entry per port (routers stay separate) and shared pure helpers in `loxone_wire` / `wire_response` only when JSON shape is identical.
3. Do **not** merge 7091 browse handlers into miniserver or vice versa without an explicit product requirement.
