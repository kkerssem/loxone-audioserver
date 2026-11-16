# Test fixtures

JSON fixtures for unit tests that assert Loxone wire shapes without a live Miniserver or speaker.

## Naming

`{feature}_{scenario}.json` — lowercase, underscores.

Examples:

| File | Used for |
|------|----------|
| `secure_init_7095.json` | Miniserver `secure/init` JWT payload |
| `geteq_zone2_flat.json` | Equalizer `geteq_result` bands |
| `loudness_get_off.json` | Loudness preference response |
| `recent_result_qobuz.json` | `recent_result` row shape for V2 `RecentlyPlayedBasedItemSchema` |
| `getroomfavs_result_tunein_qobuz.json` | Room favorites paging shell (TuneIn + legacy track row) |
| `speakers/*.json` | KEF LS60 / Naim Mu-so Qb 2 `player:player/data` captures (Cast radio, built-in radio, Qobuz Connect) and Naim `/power`; stream URLs and tokens stripped — consumed by `speaker::stream800::tests` |
| `speakers/beoplay_m3_notify_*.jsonl` | Beoplay M3 `/BeoNotify/Notifications` feed captures (own radio playing then paused, Cast stream, standby); serial in the B&O jid zeroed — consumed by `speaker::beo::tests` |
| `audioserver-config-v1.json` | Persisted config; still carries legacy Qobuz keys to prove they load and are dropped |

## Adding fixtures

1. Capture from `command-audit.log`, `trace.txt`, or a controlled test run — redact secrets.
2. Add a `source` field in the JSON when helpful (see `secure_init_7095.json`).
3. Load in tests with `include_str!` or `serde_json::from_str` in the owning module (pattern: [`src/miniserver/secure_session.rs`](../../src/miniserver/secure_session.rs)).

## Related

- Wire inventory: [docs/ui-wire-commands.md](../../docs/ui-wire-commands.md)
- Agent guide: [AGENTS.md](../../AGENTS.md)
