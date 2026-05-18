# hodl

Light crypto wallet. TUI. Rust + ratatui.

[![CI](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml/badge.svg)](https://github.com/kryptic-sh/hodl/actions/workflows/ci.yml)
[![Electrum Smoke](https://github.com/kryptic-sh/hodl/actions/workflows/smoke.yml/badge.svg)](https://github.com/kryptic-sh/hodl/actions/workflows/smoke.yml)
[![release](https://img.shields.io/github/v/release/kryptic-sh/hodl)](https://github.com/kryptic-sh/hodl/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-hodl.kryptic.sh-7ee787)](https://hodl.kryptic.sh)

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

## Install

**macOS (Homebrew)**

```bash
brew install kryptic-sh/tap/hodl
```

**Arch Linux (AUR)**

```bash
paru -S hodl-bin
```

**Alpine Linux** (once available in the apk repo)

```bash
apk add hodl
```

Until the package lands in a public Alpine repo, install the `.apk` asset from
the [GitHub Release](https://github.com/kryptic-sh/hodl/releases) directly:

```bash
apk add --allow-untrusted hodl-*.apk
```

**Pre-built binaries**

Download the tarball for your platform from the
[Releases](https://github.com/kryptic-sh/hodl/releases) page and extract the
`hodl` binary onto your `$PATH`.

## Build

```bash
cargo build --release
cargo run -p hodl
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) (if exists) or open an issue / PR.

## License

MIT. See [LICENSE](LICENSE).
