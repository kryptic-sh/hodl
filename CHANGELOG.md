# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/kryptic-sh/hodl/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.1
[0.1.0]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.0
[0.0.2]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.2
[0.0.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.1
