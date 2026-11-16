# Standard recipes — see AGENTS.md and CONTRIBUTING.md

test:
    cargo test

check:
    cargo fmt --check
    cargo clippy -- -D warnings

audit:
    cargo audit

build:
    cargo build

wire-docs:
    python3 tools/extract_ui_wire_commands.py
