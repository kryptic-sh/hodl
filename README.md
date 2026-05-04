# hodl

[![CI](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml/badge.svg)](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-hodl.kryptic.sh-7ee787)](https://hodl.kryptic.sh)

Light crypto wallet. TUI. Rust + ratatui.

## Status

Working multi-chain wallet. Send / receive / balance across all listed chains.
Roadmap tracked in [GitHub issues](https://github.com/kryptic-sh/hodl/issues).

## Goals

- Light wallet — no full-node sync, talk to public/self-hosted endpoints.
- Terminal UI via [`ratatui`](https://crates.io/crates/ratatui).
- Multi-chain: Bitcoin (+ testnet), Litecoin, Dogecoin, Bitcoin Cash, NavCoin,
  Ethereum, BNB Smart Chain, Monero. BIP-39 seed, BIP-32/44/49/84/86 derivation.
- Local-only key storage, ChaCha20-Poly1305 vault under Argon2id. Never phones
  home.
- Cross-platform: Linux, macOS, Windows.

## Layout

```
hodl/
├── apps/
│   └── hodl/                  # main binary
├── crates/
│   ├── hodl-core/             # shared types, errors, traits
│   ├── hodl-config/           # config + known_hosts loading (TOML)
│   ├── hodl-wallet/           # vault, BIP-39, BIP-32 derivation, signing
│   ├── hodl-chain-bitcoin/    # BTC + LTC + DOGE + BCH + NAV (Electrum)
│   ├── hodl-chain-ethereum/   # ETH + BSC (JSON-RPC, EIP-1559)
│   ├── hodl-chain-monero/     # XMR (LWS)
│   └── hodl-tui/              # ratatui screens, input, layout
└── Cargo.toml                 # workspace root
```

## Build

```bash
cargo build --release
cargo run -p hodl
```

## License

MIT. See [LICENSE](LICENSE).
