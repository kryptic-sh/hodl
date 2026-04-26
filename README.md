# hodl

[![CI](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml/badge.svg)](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-hodl.kryptic.sh-7ee787)](https://hodl.kryptic.sh)

Light crypto wallet. TUI. Rust + ratatui.

## Status

Early scaffold. Not usable yet.

## Goals

- Light wallet — no full-node sync, talk to public/self-hosted RPC endpoints.
- Terminal UI via [`ratatui`](https://crates.io/crates/ratatui).
- Multi-chain (start: Bitcoin, Ethereum). BIP-39 seed, BIP-32/44 derivation.
- Local-only key storage. Encrypted at rest. Never phones home.
- Cross-platform: Linux, macOS, Windows.

## Layout

```
hodl/
├── apps/
│   └── hodl/           # main binary
├── crates/
│   ├── hodl-core/      # shared types, errors, traits
│   ├── hodl-tui/       # ratatui screens, input, layout
│   ├── hodl-config/    # config loading (TOML)
│   └── hodl-wallet/    # keys, addresses, signing, RPC
└── Cargo.toml          # workspace root
```

## Build

```bash
cargo build
cargo run -p hodl
```

## License

MIT. See [LICENSE](LICENSE).
