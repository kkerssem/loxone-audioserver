# Command routing: port 7095 vs 7091

Some Loxone clients use **both** WebSockets during a session; others may send media commands only on one port. Do not assume “UI always uses 7091” without checking your firmware/app build.

## Ports

| Port | Module | Typical clients |
|------|--------|-----------------|
| **7095** | `miniserver` | Loxone Config (pairing), Miniserver upstream, **some App/Web UI builds** (transport on same socket as config) |
| **7091** | `remote` | Direct App/Web UI (`remotecontrol` subprotocol), debugging |

Historical captures: [`trace-7095.txt`](../reference/loxone-ui/trace-7095.txt) (pairing-heavy), [`trace.txt`](../reference/loxone-ui/trace.txt) (7091 bootstrap). Re-capture after firmware upgrades.

## Command routing matrix

**Source:** local `command-audit.log` (119 commands in one capture, `LOXONE_AUDIT=1`) — Loxone **mobile + desktop** UI, same LAN session. The file is **ephemeral** (gitignored, written to the serve working directory); re-capture after firmware upgrades.

| Command group | Examples | Expected (older docs) | Observed (audit) | Server accepts |
|---------------|----------|----------------------|------------------|----------------|
| Pairing / TLS | `secure/info/pairing`, `secure/hello`, `secure/init` | 7095 | **7095 only** | **7095 only** |
| Zone config sync | `getconfig`, `volumes`, `playername`, `playeropts`, `ready`, `miniservertime` | 7095 | **7095 only** | **7095 only** |
| Bootstrap | `getkey/full`, `authenticate`, `getservices`, `getradios` | 7091 | **7091 only** (4 reconnect bursts ≈ 4 UI sessions). `getservices` now returns `[]`. | **7091 only** |
| Browse / library | `getservicefolder`, `globalsearch`, `getplaylists2`, `getmediafolder`, `recent`, `storage/*` | 7091 | **7091 only** — radio browse via `getservicefolder`; **network folders / NAS not implemented** ([web-ui-protocol.md § Local library and network folders](web-ui-protocol.md#local-library-and-network-folders-not-implemented)); `getplaylists2` returns an empty stub | **7091 only** |
| Play / queue skip | `play`, `queueplus`, `queueminus` | 7091 | **7095** (primary in capture) | **7091 + 7095** via [`zone_transport`](../src/zone_transport.rs) |
| Resume (unpause) | `resume` | — | **7095** | **7091 + 7095** |
| Pause | `pause` | 7091 | **7095** (e.g. `audio/1/pause` in audit) | **7091 + 7095** |
| Zone off (Audio Player **Off** input, scenes) | `pause` | — | **7095** — sent as `audio/{id}/pause`, indistinguishable from a UI pause (verified 2026-11-14 with Off wired to "leaving room"); no dedicated power/off command exists | **7091 + 7095** |
| Volume UI (+/−) | `volume/+1`, `volume/-1` | 7091 | **7091 only** | **7091 + 7095** |
| Volume absolute | `volume/{n}` (slider / structure) | unclear | **7095 only** (+ matching `audio/cfg/volumes`) | **7091 + 7095** |
| Repeat | `repeat/{mode}` | 7091 | **7091 only** | **7091 + 7095** |
| Seek | `position/{seconds}` | 7091 | **7091 only** — wire value is seconds | **7091 + 7095** |
| Queue browse | `getqueue` | 7091 | **7091** (primary UI path) | **7091 + 7095** — empty queue (`remote` router; 7095 via [`try_dispatch_getqueue`](../src/remote/mod.rs) fallback) |
| Zone loudness | `loudness`, `loudness/0`, `loudness/1` | 7091 | **7091 only** | **7091 + 7095** |
| Zone equalizer | `geteq`, `seteq`, `equalizer` | 7091 | **7091 only** (e.g. `audio/cfg/geteq/2` in audit) | **7091 + 7095** |
| EQ transfer (“Apply settings to…”) | `seteq` per target zone on ✓ only | 7091 | Zone list is **Loxone structure**, not audioserver — see [web-ui-protocol.md § Transfer EQ](web-ui-protocol.md#transfer-eq-apply-settings-to) | **7091 + 7095** |
| Status | `status` | 7091 | *not seen in this capture* | **7091 + 7095** |
| TTS / announcements | `audio/grouped/tts/...` | — | **7095 only** (not in `zone_transport` yet) | **7095 only** (handler not implemented) |
| State push | `audio_event` (server → client) | both (broadcast) | **both** (clients on 7091 and 7095) | **both** |
| HW heartbeat | `hw_event` | 7095 only | **7095 only** | **7095 only** |

**Takeaway:** One UI session uses **both** ports in parallel — **7091** for the music app (bootstrap, browse, vol+/−, repeat, seek) and **7095** for play/pause/skip and structure-driven volume. Not “7091 only” for transport in the audit.

**Server implementation:** Shared [`zone_transport.rs`](../src/zone_transport.rs) is dispatched from **both** [`remote/mod.rs`](../src/remote/mod.rs) (primary 7091 transport routes) and [`miniserver/mod.rs`](../src/miniserver/mod.rs) (fallback when the config router does not match). `getqueue` uses the same 7095 fallback path via [`try_dispatch_getqueue`](../src/remote/mod.rs).

## Capture procedure

1. Run with info logging:

   ```bash
   RUST_LOG=loxone_audioserver=info cargo run --release -- --serial YOUR_SERIAL
   ```

2. Optional append-only audit file (one line per command: `port<TAB>command`):

   ```bash
   LOXONE_AUDIT=1 RUST_LOG=loxone_audioserver=info cargo run --release -- --serial YOUR_SERIAL
   ```

   Output: `command-audit.log` in the working directory.

3. Exercise the UI: cold start, play/pause/volume/skip, zone detail, second client/tab, Config-only edit.

4. Sort logs:

   ```bash
   grep 'loxone inbound command'   # structured: port, session_token, command
   # or
   sort command-audit.log | cut -f1 | uniq -c
   ```

Each inbound line is logged by [`src/command_audit.rs`](../src/command_audit.rs) with `port` **7095** or **7091**.

## Connect-time `audio_event`

New WebSocket clients receive a per-zone snapshot on connect (registry state, or an idle shell):

| Port | Implementation |
|------|----------------|
| 7091 | [`remote/ws.rs`](../src/remote/ws.rs) → [`resync_connect_audio_events`](../src/connect_bootstrap.rs) |
| 7095 | [`miniserver/ws.rs`](../src/miniserver/ws.rs) connect burst → [`fetch_connect_player_states`](../src/connect_bootstrap.rs) |

Ongoing updates are **event-driven**: producers commit to [`ZoneRegistry`](../src/zone_registry.rs) and [`zone_publish`](../src/zone_publish.rs) broadcasts `IoEvent::State` → `audio_event` when mode, title, duration, cover, `audiopath` or volume change. The dedup key **omits `time`** so the mini player can advance the scrubber locally every second during play.

### Scrubber position and `audiopath`

The Loxone web UI (`comps.js` `useCurrentlyPlayedTime`) **does not apply** server `time: 0` on updates (`if (playerState.time)` treats zero as falsy). On a track change the scrubber resets when **`audiopath` changes**. Opening the full-player popup remounts the hook, which initializes from the last `audio_event` `time`, so producers should send a periodic `time` while playing.

## Related docs

- [Miniserver protocol (7095)](miniserver-protocol.md)
- [Web UI protocol (7091)](web-ui-protocol.md)
- [Radio over Cast](radio-cast.md)
