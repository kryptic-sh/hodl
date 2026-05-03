# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- M2 TUI surfaces: onboarding (create + restore), accounts list, receive (QR +
  OSC-52 clipboard yank), settings — all built on `hjkl-form` (modal vim grammar
  inside every field) and `hjkl-picker` (chain / endpoint switchers).
- Workspace deps `hjkl-picker = "0.3"`, `hjkl-clipboard = "0.4"`,
  `qrcode = "0.14"`.
- `crates/hodl-tui/src/`: `app.rs` (app state machine), `account.rs` (Bitcoin
  accounts table + chain picker overlay), `receive.rs` (QR + clipboard yank),
  `settings.rs` (settings form, on-save disk write), `onboarding.rs` (create +
  restore flows), `clipboard.rs` (thin `hjkl-clipboard` wrapper).

### Changed

- `hodl init` now drops into the modal-form onboarding TUI instead of the
  line-prompted flow. Mnemonic display + write-down gate happen inside the
  alt-screen. A new `hodl restore` subcommand triggers the restore flow
  directly.
- `hodl_tui::lock::Outcome` gains `Unlocked(UnlockedWallet)` so the lock screen
  hands the unlocked wallet to the app loop instead of rendering its M1
  placeholder. `Mode::Unlocked` rendering path and the manual `l` lock toggle
  are removed from `lock.rs`.

- New crate `hodl-chain-bitcoin`: Electrum 1.4 client (TCP + TLS), BIP-44 / 49 /
  84 / 86 address derivation (P2PKH, P2SH-P2WPKH, P2WPKH bech32, P2TR bech32m),
  gap-limit scan, balance + history read path. `BitcoinChain` implements
  `hodl_core::Chain` for the read path; `build_tx` / `sign` / `broadcast` return
  `Error::Chain("not implemented")` until PE (M3 send).
- Network constants: `NetworkParams::BITCOIN_MAINNET` and `BITCOIN_TESTNET`.
- `hodl-core` types: `ChainId`, `Address`, `Amount`, `FeeRate`, `TxId`, `TxRef`,
  `SendParams`, `UnsignedTx`, `SignedTx`, `PrivateKeyBytes` (Zeroize), and the
  `Chain` trait per `PLAN.md`.
- `hodl-config` endpoint registry: per-chain `endpoints` (Electrum / JSON-RPC /
  LWS), `tor`, `lock.idle_timeout_secs`, `kdf` preset. Loader returns in-memory
  defaults on missing file — never auto-writes.
- Workspace deps `hjkl-form = "0.3"` (with `crossterm` feature) and
  `hjkl-ratatui = "0.3"`.
- Bumped workspace pins `ratatui` 0.28 → 0.30 and `crossterm` 0.28 → 0.29.

### Changed

- `hodl-tui::lock` now uses `hjkl-form` for password entry (carried from prior
  phase). Password is masked on render and zeroized on every unlock attempt.

## [0.1.2] - 2026-05-03

### Fixed

- Release workflow now adds the matrix target std explicitly via
  `rustup target add` after the `dtolnay/rust-toolchain` step. The action's
  `targets:` input was not actually adding `x86_64-apple-darwin` std on the
  arm64 macOS runner — `rustup toolchain install` saw the toolchain as
  already-installed and skipped the target. The Intel-mac binary failed to build
  in 0.1.0 and 0.1.1 as a result.

## [0.1.1] - 2026-05-03

### Fixed

- Release workflow now skips the `cargo fmt --check` and `cargo clippy` steps in
  the build matrix — those run in CI on every push to main. The redundant Clippy
  step was failing on `x86_64-apple-darwin` (target std issue), which prevented
  the Intel-mac binary from being published in 0.1.0. Aligns with the canonical
  sqeel / hjkl release.yml pattern (build-only).

## [0.1.0] - 2026-05-03

### Changed

- `hodl --help` now shows ASCII-art branding plus the package version inline in
  the long-form help. `--version` continues to print the version on its own.
  Mirrors the cross-project CLI standardization.
- **Vault path resolution migrated to `hjkl-config` 0.2 (XDG-everywhere).**
  `hodl_wallet::storage::default_data_dir()` now routes through
  `hjkl_config::data_dir("hodl")` instead of
  `directories::ProjectDirs::from("sh", "kryptic", "hodl")`. macOS users move
  from `~/Library/Application Support/sh.kryptic.hodl/wallets/` to
  `~/.local/share/hodl/wallets/`. Windows users move from
  `%APPDATA%\kryptic\hodl\data\wallets\` to `~/.local/share/hodl/wallets/`.
  Linux paths unchanged. Replaced `directories` workspace dep with
  `hjkl-config = "0.2"`.

### Added

- CLI smoke tests: `--version` returns `CARGO_PKG_VERSION`, long-form help
  contains the ASCII art and the version.

## [0.0.2] - 2026-04-26

### Added — M1 wallet core

- BIP-39 mnemonic generation (12 / 24 words, English wordlist) and parsing with
  strict word-count validation.
- BIP-39 seed derivation (PBKDF2-HMAC-SHA512, 2048 iters, optional passphrase).
- BIP-32 hierarchical deterministic key derivation via the `bip32` crate; master
  key from 64-byte seed, hardened + non-hardened child derivation, BIP-44 path
  helper (`m/44'/coin'/account'/change/index`).
- Encrypted vault file format
  `magic("HODLVLT\0") | version(2) | argon2_params(16) | salt(16) | nonce(12) | ciphertext | tag(16)`.
  Argon2id KDF (default `m=64 MiB, t=3, p=1`), ChaCha20-Poly1305 AEAD with the
  full header bound as associated data.
- `hodl_wallet::Wallet` / `UnlockedWallet` API: vault create / open / unlock,
  derivation through to `XPrv`, `Zeroize` / `ZeroizeOnDrop` discipline on all
  seed-bearing types.
- Vault storage under `$XDG_DATA_HOME/hodl/wallets/<name>.vault` via
  `directories::ProjectDirs`.
- Ratatui lock screen (`hodl-tui`) with masked password entry, error feedback,
  manual lock, and idle auto-lock (default 5 minutes).
- `hodl init` and `hodl unlock` CLI subcommands (binary crate `hodl`).

### Tests

- BIP-39 Trezor vectors (5 entries, 12-word + 24-word, with `TREZOR` passphrase)
  — round-trip mnemonic and 64-byte seed.
- BIP-32 vectors 1 and 2 (master + interior derivation paths) against canonical
  `xprv` strings.
- Vault round-trip, wrong-password rejection, ciphertext tamper rejection,
  header (AAD) tamper rejection, deterministic encrypt with fixed salt + nonce,
  magic-byte rejection, KDF-params byte round-trip.
- Wallet create / open / unlock round-trip and overwrite / missing-vault guards.
  All disk activity uses `tempfile::TempDir`.

### Notes

- `hjkl` integration is intentionally deferred; the workspace
  `[patch.crates-io]` block is staged but unused. Lock-screen input goes through
  plain `crossterm` + `ratatui` for M1.

## [0.0.1] - earlier

- Workspace scaffold (M0): crates, CI lint/build/test on Linux.

[Unreleased]: https://github.com/kryptic-sh/hodl/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.2
[0.1.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.1
[0.1.0]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.0
[0.0.2]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.2
[0.0.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.1
