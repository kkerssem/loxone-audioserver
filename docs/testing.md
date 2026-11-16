# Testing and coverage

## Running tests

```bash
cargo test
```

Disk-backed unit tests (`config/`, `recents/`, `private.pem` via `with_test_keys`) redirect persistence to a temporary directory under `std::env::temp_dir()` via test-only path overrides — they never write production `audioserver-config.json`, `recents/`, or `private.pem`.

## Coverage tiers

Line coverage uses [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov). **Use a rustup-managed `rustc`** (not Homebrew Rust) and install `llvm-tools-preview` on that toolchain:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

| Tier | Target | Modules |
|------|--------|---------|
| **Overall in-scope** | **≥80%** | Weighted line coverage across all non–Tier C files (see [`scripts/coverage.sh`](../scripts/coverage.sh) ignore list) |
| **A — Authority / pure logic** | ≥90% per file | `zone_registry`, `zone_publish`, `zone_enrich`, `wire_response`, `config/{mod,loxone,zone,crc}`, `player_state`, `recents`, `radio/state`, `radio/search_cache`, `speaker/{map,binding}` |
| **B — Orchestration / adapters** | ≥80% average | `coordinator/{mod,speaker}`, `connect_bootstrap`, `zone_transport`, `remote/router`, `miniserver/router`, `cast/sync`, `speaker/manager` |
| **C — Excluded** | No gate | I/O shells, cloud loops, mDNS, OAuth HTTP — see table below |

Run locally:

```bash
./scripts/coverage.sh
```

Reports: `target/llvm-cov/lcov.info` (gates) and `target/llvm-cov/cobertura.xml` (CI job summary / PR comments via CodeCoverageSummary).

## Gates

[`scripts/coverage.sh`](../scripts/coverage.sh) runs tests with Tier C paths ignored, then [`scripts/coverage-check-files.sh`](../scripts/coverage-check-files.sh) enforces:

1. **Overall in-scope ≥80%**
2. **Tier A** ≥90% per file
3. **Tier B** ≥80% average (per-file warnings only)

The check script prints the top 10 uncovered in-scope files to guide further work.

## Coverage exceptions (Tier C rationale)

| File / area | Rationale |
|-------------|-----------|
| `src/main.rs` | Daemon entry and task spawn |
| `src/miniserver/mod.rs`, `src/miniserver/ws.rs` | 7095 WS loop; crypto in `secure_session` |
| `src/coordinator/{transport,cast,browse,config,status}.rs` | Live session / device orchestration |
| `src/cast/manager.rs`, `src/cast/session.rs` | Live Cast receiver |
| `src/remote/mod.rs`, `src/remote/ws.rs`, `src/remote/handlers/*` | WebSocket I/O shell; routes tested via `remote/router` |
| `src/mdns_browse.rs`, `src/discovery/mdns.rs` | mDNS — flaky in CI |
| `src/radio/tunein_*.rs`, `src/radio/audiopath.rs`, `src/radio/custom_streams.rs` | TuneIn HTTP / filesystem I/O |
| `src/cast/discovery.rs`, `src/tls.rs` | mDNS stub, TLS stub |

## Key test suites

| Module | Focus |
|--------|--------|
| `src/zone_registry.rs` | Invariants, dedup, Cast grace, merge/scrub normalization |
| `src/zone_publish.rs` | Registry commit + `IoEvent::State` fan-out |
| `src/coordinator/mod.rs` | IoCommand harness (Status, browse, config, transport) |
| `src/connect_bootstrap.rs` | Lag-resync payloads, configured player ids |
| `src/config/` | Unified config load/save, wire CRC, Loxone + audioserver sections |
| `src/recents.rs` | Recent list dedup/cap/pagination; disk tests use `with_test_recents_dir` |
| `src/miniserver/secure_session.rs` | 7095 pairing crypto |
| `src/zone_transport.rs` | Route parsing (7091 + 7095) |

Wire JSON fixtures: [`tests/fixtures/`](../tests/fixtures/) — see [`tests/fixtures/README.md`](../tests/fixtures/README.md).
