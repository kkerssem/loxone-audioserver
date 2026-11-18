# Agent guide (loxone-audioserver)

Software Loxone Audioserver for LAN speakers: per-speaker control (KEF W2 / Naim Mu-so 2nd gen) and Cast radio. Human setup: [README.md](README.md). Architecture: [docs/architecture.md](docs/architecture.md). Doc index: [docs/README.md](docs/README.md).

## Build and test (required)

The `miniserver` feature is enabled by default. Test protocol changes with:

```bash
cargo test
```

Other common commands:

```bash
cargo build
cargo run -- --serial YOUR_SERIAL
just test      # cargo test
just check     # fmt + clippy -D warnings
just audit     # cargo audit (also in CI)
```

CI (`.github/workflows/ci.yml`) runs `fmt`, instrumented tests/coverage gates, and `cargo audit` on push/PR; `clippy -D warnings` on PRs only.

## Module map

| Area | Path | Role |
|------|------|------|
| Entry / daemon | [`src/main.rs`](src/main.rs) | Flat-args daemon: discovery, miniserver, remote, coordinator |
| Coordinator hub | [`src/coordinator/`](src/coordinator/) | `IoCommand` / `IoEvent` loop, browse, config, transport dispatch |
| Zone authority | [`src/zone_registry.rs`](src/zone_registry.rs) | Sync per-zone state, invariants, dedup |
| Zone publish | [`src/zone_publish.rs`](src/zone_publish.rs) | Commit registry + fan-out `IoEvent::State` (`audio_event`) |
| Zone presentation | [`src/zone_enrich.rs`](src/zone_enrich.rs) | Cover proxy, Loxone UI envelope, Cast status enrichment |
| Connect bootstrap | [`src/connect_bootstrap.rs`](src/connect_bootstrap.rs) | Connect-time / lag-resync snapshots via Status |
| 7091 Web UI | [`src/remote/`](src/remote/) | [`mod.rs`](src/remote/mod.rs) types + dispatch; [`ws.rs`](src/remote/ws.rs) Axum/WS; [`handlers/`](src/remote/handlers/) command handlers; [`router.rs`](src/remote/router.rs) route table; 7091 auth stubs in [`handlers/secure.rs`](src/remote/handlers/secure.rs) (pairing/crypto on 7095) |
| 7095 Miniserver | [`src/miniserver/`](src/miniserver/) | [`mod.rs`](src/miniserver/mod.rs) handlers; [`ws.rs`](src/miniserver/ws.rs) Axum/WS; [`router.rs`](src/miniserver/router.rs) route table; pairing, config sync, secure session; transport/`getqueue` fallback |
| Shared wire helpers | [`src/loxone_wire.rs`](src/loxone_wire.rs) | `not_implemented`, base64 config decode, stripped command echo — see [docs/wire-handler-overlap.md](docs/wire-handler-overlap.md) |
| Zone transport (7091 + 7095) | [`src/zone_transport.rs`](src/zone_transport.rs) | play/pause/volume/seek via shared `zone_transport::dispatch` |
| Persisted config | [`src/config/`](src/config/) | `audioserver-config.json` load/save, wire CRC, Loxone + audioserver sections, zone mapping sync ([`mapping.rs`](src/config/mapping.rs)) |
| Loxone config wire (shim) | [`src/loxone_config.rs`](src/loxone_config.rs) | Re-exports [`src/config/loxone.rs`](src/config/loxone.rs) |
| Zone equalizer | [`src/zone_equalizer.rs`](src/zone_equalizer.rs) | Per-zone EQ wire handlers |
| Zone audio prefs | [`src/zone_audio_prefs.rs`](src/zone_audio_prefs.rs) | Loudness preference persistence |
| Speaker control | [`src/speaker/`](src/speaker/) | `SpeakerBackend` trait, status → `PlayerState` mapper, mDNS discovery, zone binding, polling manager; KEF/Naim HTTP backend in [`stream800.rs`](src/speaker/stream800.rs) (quirks: `pause` is a toggle and `play` does not resume; live radio has no `pause` control (a KEF answers `pause` with HTTP 500), so it is stopped (and shown as paused) and `play` restarts it — Naim via 15081 `GET /nowplaying?cmd=play`, otherwise by replaying the stopped stream's `mediaRoles`; KEF app-standby reads as source `powerOn`, and waking means selecting an input); B&O BeoLink/ASE (Beoplay M3) backend in [`beo.rs`](src/speaker/beo.rs) (state from the `/BeoNotify/Notifications` long-poll feed; volume range 0–90 and ignored while paused; commands serialized with a long timeout, the speaker stalls); coordinator glue in [`coordinator/speaker.rs`](src/coordinator/speaker.rs) |
| Google Cast | [`src/cast/`](src/cast/) | Cast discovery, radio playback session, remote volume/transport sync |
| Voice messages | [`src/voice_upload.rs`](src/voice_upload.rs), [`coordinator/voice.rs`](src/coordinator/voice.rs), [`cast/announce.rs`](src/cast/announce.rs) | App recording: `PUT /audio/cfg/upload/audioupload/add/{file}` (7091) stores the WAV in `voice-uploads/`; `audio/grouped/playuploadedfile/{file}/{zones}` casts it once per zone (TTS volume as minimum), then restarts our Cast radio or the speaker's own live radio (a track stays stopped) |
| Radio | [`src/radio/`](src/radio/) | TuneIn browse/search, custom streams, radio audiopaths |
| Player wire shape | [`src/player_state.rs`](src/player_state.rs) | `PlayerState` / `audio_event` |
| Wire envelopes | [`src/wire_response.rs`](src/wire_response.rs) | `{name}_result` before `command` |

Full tables and sequence diagrams: [docs/architecture.md](docs/architecture.md).

## Zone state placement checklist

When adding or changing playback state behaviour, use this table:

| Question | Module |
|----------|--------|
| Who owns the zone? Invariants? Dedup? | [`zone_registry.rs`](src/zone_registry.rs) |
| Live Cast session / listener? | [`cast/manager.rs`](src/cast/manager.rs) |
| Speaker poll / command? | [`speaker/manager.rs`](src/speaker/manager.rs), mapping in [`speaker/map.rs`](src/speaker/map.rs) |
| Cover proxy, EQ, Loxone idle shells? | [`zone_enrich.rs`](src/zone_enrich.rs) |
| Commit + fan-out `audio_event`? | [`zone_publish.rs`](src/zone_publish.rs) |
| `IoCommand` loop, browse, config, side effects? | [`coordinator/`](src/coordinator/) |
| Connect bootstrap / lag resync? | [`connect_bootstrap.rs`](src/connect_bootstrap.rs) |

**Hard rules:**

1. `zone_registry` stays sync — no tokio, Cast, speaker, or filesystem I/O.
2. Only the coordinator task mutates `ZoneRegistry`.
3. Producers never call `events.send(IoEvent::State)` except via `zone_publish`.
4. [`refresh_zone_from_backends`](src/coordinator/status.rs) is the only documented Status cold-start bypass (polls Cast when the registry slot is empty).

[`CastPlaybackManager`](src/cast/manager.rs) owns the live Cast device session; [`ZoneRegistry`](src/zone_registry.rs) holds the Loxone wire snapshot.

## Where to change what

| Task | Start here |
|------|------------|
| New **7091** command | [`src/remote/mod.rs`](src/remote/mod.rs) router + [`IoCommand`](src/remote/mod.rs) + [`src/coordinator/`](src/coordinator/) handler |
| **7091 + 7095** transport / `getqueue` fallback | [`src/remote/mod.rs`](src/remote/mod.rs), [`src/miniserver/mod.rs`](src/miniserver/mod.rs), [`src/zone_transport.rs`](src/zone_transport.rs), [`src/coordinator/transport.rs`](src/coordinator/transport.rs) |
| Radio / Cast playback | [`src/radio/`](src/radio/), [`src/cast/`](src/cast/), [docs/radio-cast.md](docs/radio-cast.md) |
| Speaker control (KEF / Naim / B&O) | [`src/speaker/`](src/speaker/), [docs/speakers/](docs/speakers/README.md) — speaker API reference |
| Wire format / UI expectations | [docs/web-ui-protocol.md](docs/web-ui-protocol.md) |
| Zone ↔ Loxone mapping | [`src/config/mapping.rs`](src/config/mapping.rs) |
| 7091 vs 7095 routing | [docs/traffic-paths.md](docs/traffic-paths.md) |

## Constraints

- **`miniserver` feature** gates Loxone modules in [`src/main.rs`](src/main.rs); use `#[cfg(feature = "miniserver")]` for new Loxone-only code.
- **No streaming services:** `getservices` / `getavailableservices` return `[]`; Qobuz-era commands (`servicecfg`, `isfollowed`, `queue/*`, `library/play`, …) are kept as routes that return empty stubs so the UI does not error.
- **Do not commit:** `audioserver-config.json`, legacy `zone-config.json` / `qobuz-config.json`, tokens, `state.json`, `recents/`, `voice-uploads/`, audit logs.
- **Minimal diffs:** match existing `tracing`, `IoCommand`, `build_wire_result_response` patterns.
- **`getqueue`:** answered locally with an empty queue on both ports ([`build_getqueue_response`](src/remote/mod.rs)).

## Logging

Use [`tracing`](https://docs.rs/tracing) only — no `println!` / `eprintln!`. Import macros per module:

```rust
use tracing::{debug, info, warn, error};
```

Canonical shape (fields first, message last) — see [`command_audit.rs`](src/command_audit.rs):

```rust
info!(port = 7091, api = "remote", session_token, "remote websocket connected");
info!(port, api, session_token, command, "loxone inbound command");
warn!(player_id, error = %e, "failed to persist cast volume");
error!(port = 7095, error = %e, "miniserver api stopped");
debug!(player_id, "connect status skipped: no renderer mapped");
```

| API endpoint | Port | `api` field | Message stem |
|--------------|------|-------------|--------------|
| Remote (Loxone App mobile/desktop, Web UI builds) | 7091 | `remote` | `remote api …`, `remote websocket …` |
| Miniserver upstream (Config pairing, `setconfig`) | 7095 | `miniserver` | `miniserver api …`, `miniserver websocket …` |

Routing reference: [docs/traffic-paths.md](docs/traffic-paths.md).

| Rule | Detail |
|------|--------|
| Errors | `error = %e` (Display); not bare `%error` / `?error` for `anyhow::Error` |
| Strings | `%field` for display strings; bare names for `Copy` numerics |
| Messages | lowercase, short, no trailing period |
| `error!` | handler/task cannot complete; fatal connection/server stop |
| `warn!` | fallback, degraded mode, ignored I/O |
| `info!` | lifecycle outcomes (start/stop, connect/disconnect, config saved, bridge up/down) |
| `debug!` | diagnostics only; no full wire JSON on hot paths |

Do not add logs inside [`zone_registry.rs`](src/zone_registry.rs) (sync hot path).

## Debugging

Default logging: `info`, `mdns_sd=off`. Override when investigating:

```bash
RUST_LOG=loxone_audioserver=debug cargo run -- --serial YOUR_SERIAL
RUST_LOG=loxone_audioserver::cast=debug,loxone_audioserver::coordinator=debug cargo run -- --serial YOUR_SERIAL
LOXONE_AUDIT=1 cargo run -- --serial YOUR_SERIAL   # command-audit.log
```

Capture matrix: [docs/traffic-paths.md](docs/traffic-paths.md).

## Tests and fixtures

- Unit tests live inline (`#[cfg(test)]`) in modules.
- Wire JSON fixtures: [`tests/fixtures/`](tests/fixtures/) — see [tests/fixtures/README.md](tests/fixtures/README.md). Speaker API captures live in `tests/fixtures/speakers/` (stream URLs and per-device tokens stripped).
- Tests that touch the process-wide config must hold the config test lock: `AudioserverConfigFile::with_test_cache` / `with_test_disk`, or `lock_for_test()` in async tests.
- Read-only check against a real KEF/Naim speaker: `STREAM800_LIVE=<ip>:kef|naim cargo test live_status -- --ignored --nocapture` (run from a terminal with macOS local-network access).
- Regenerate wire command index after `scripts6.js` updates: `just wire-docs` or `python3 tools/extract_ui_wire_commands.py`.
- Regenerate V2 schema name inventory after `comps.js` updates: `python3 tools/extract_v2_schemas.py` (requires local `reference/loxone-ui/comps.js`).

## Reference assets (read-only)

Minified Loxone UI bundles and captures live under [`reference/loxone-ui/`](reference/loxone-ui/). Prefer **docs/** and **tests/fixtures/** over parsing megabyte JS files unless investigating UI behaviour.

When touching Tier A/B modules ([`docs/testing.md`](docs/testing.md)), run `./scripts/coverage.sh` — overall in-scope line coverage must stay **≥80%**.

## Manual-only verification

Real speakers, Loxone App/Config, and live Cast playback require hardware — use unit tests + fixtures in agent workflows; document manual steps in PR test plans.
