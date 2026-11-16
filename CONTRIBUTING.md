# Contributing

Thank you for improving the Loxone Audioserver. Agents and humans should read [AGENTS.md](AGENTS.md) and [docs/architecture.md](docs/architecture.md) first.

## Prerequisites

- Rust (stable) — [rustup](https://rustup.rs/)
- Optional: [just](https://github.com/casey/just) for standard recipes (`just test`, `just check`)

## Before opening a PR

```bash
just test          # or: cargo test
just check         # fmt --check + clippy -D warnings
just audit         # optional: cargo audit (also runs in CI)
```

CI runs `fmt`, instrumented tests/coverage, and `cargo audit` on push/PR to `main`; `clippy -D warnings` on pull requests only. Advisory ignores live in [`.cargo/audit.toml`](.cargo/audit.toml).

## Code conventions

- Loxone integration code belongs behind `#[cfg(feature = "miniserver")]`.
- New wire commands: route in [`src/remote/router.rs`](src/remote/router.rs), `IoCommand` variant in [`src/remote/mod.rs`](src/remote/mod.rs), handler in [`src/coordinator/`](src/coordinator/mod.rs).
- Reply envelopes: use [`build_wire_result_response`](src/wire_response.rs) (`{name}_result` before `command`).
- Prefer unit tests in the same module; wire shapes can use JSON under [`tests/fixtures/`](tests/fixtures/).
- Logging: [`tracing`](https://docs.rs/tracing) only; follow the **Logging** section in [AGENTS.md](AGENTS.md).

## Wire command inventory

If you update your local copy of `reference/loxone-ui/scripts6.js` (Loxone media bundle, not in the repo), regenerate:

```bash
just wire-docs
```

Commit changes to [`docs/ui-wire-commands.md`](docs/ui-wire-commands.md) and [`docs/ui-wire-commands.json`](docs/ui-wire-commands.json) when paths or `MediaEnum` literals change.

## Do not commit

- `audioserver-config.json`, legacy `zone-config.json` / `qobuz-config.json`, OAuth tokens, PEM keys
- `state.json`, `state.raw`, `recents/`, `command-audit.log`, local logs

## Reference file moves

Loxone UI dumps live under [`reference/loxone-ui/`](reference/loxone-ui/). Update doc links to `../reference/loxone-ui/…` when citing `scripts6.js`, `comps.js`, or `trace.txt`.

## Manual testing

Real Loxone App, Miniserver pairing, and LAN speakers are not required for most PRs. Describe manual steps in the PR **Test plan** when behaviour is client- or hardware-specific.
