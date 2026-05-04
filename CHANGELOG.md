# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-05-04

Chain enum reshape + curated default Electrum endpoints. Two breaking changes
(`ChainId::Navio` → `NavCoin`, drop BSV + XEC) plus the first shipped server
list — wallet now works out of the box for the BTC family without a hand-written
`config.toml`.

### Changed

- **BREAKING:** rename `ChainId::Navio` → `ChainId::NavCoin` (ticker `NAV`,
  display "NavCoin"). What v0.2.0 shipped as "Navio" was actually NavCoin
  network params (P2PKH prefix 0x35 → "N", SLIP-44 130). Bech32 HRP corrected
  from `"navio"` to `"nav"`. Default Electrum ports moved to 40001 / 40002 to
  match upstream electrumx-NAV. `NetworkParams::NAVIO_MAINNET` →
  `NAVCOIN_MAINNET`. The newer **Navio** chain (Navio-project, distinct fork) is
  deferred to a future release as a sibling `NetworkParams` record.

### Added

- Curated default Electrum endpoint list ships in `Config::default()` for every
  BTC-family chain (BTC mainnet + testnet, BCH, LTC, DOGE, NAV). Five TLS-only
  servers per chain. Sourced from the `1209k.com/bitcoin-eye` reliability
  monitor on 2026-05-04 (NavCoin servers from upstream). The wallet still does
  not phone home on its own — endpoints are only contacted when the user opens
  accounts / receive / send. EVM (ETH/BSC) and Monero remain endpoint-empty by
  default: EVM needs a per-user API key and Monero LWS leaks the view key to the
  operator (privacy-conservative default = self-host).
- `Config::load` now merges user `[chains.X]` overrides **per chain key** over
  the curated defaults: writing your own `[chains.bitcoin]` only replaces BTC,
  every other chain keeps its default endpoint list. Closes a footgun where a
  bare-bones user config silently dropped every default it didn't restate.

### Removed

- **BREAKING:** dropped Bitcoin SV (BSV) and eCash (XEC). Both sit deep outside
  the CMC top-100 (BSV #113, ~$320M; XEC #167, ~$145M) with little active
  development; carrying their constants + tests was buying nothing. Removed:
  `ChainId::BitcoinSv`, `ChainId::ECash`, `NetworkParams::BITCOIN_SV_MAINNET`,
  `NetworkParams::ECASH_MAINNET`, the per-chain branches in `address.rs` and
  `derive.rs::validate_purpose`, the address-book aliases, and the related
  tests. CashAddr encoder stays for BCH. If demand returns, re-add as
  `NetworkParams` records — the abstraction makes it cheap.

## [0.2.0] - 2026-05-04

End-to-end M2 → M8 release. Ships read + send for Bitcoin and Ethereum, read for
Monero, read paths for the BTC-derivative family (LTC, DOGE, BCH, BSV, XEC,
Navio) and BSC, plus the modal-form TUI past the lock screen and Tor passthrough
across every backend.

### Added

#### Chain support

- New crate `hodl-chain-bitcoin`: Electrum 1.4 client (TCP + TLS via rustls +
  webpki-roots), BIP-44 / 49 / 84 / 86 address derivation (P2PKH, P2SH-P2WPKH,
  P2WPKH bech32, P2TR bech32m with BIP-341 key-path tweak via `k256`), gap-limit
  scan, balance + history read path. Hand-rolled BIP-143 segwit sighash, PSBT v0
  build (segwit-v0 P2WPKH only), greedy coin selection, k256 ECDSA sign,
  broadcast via Electrum. `NetworkParams` records cover Bitcoin mainnet/testnet,
  Litecoin, Dogecoin, Bitcoin Cash, Bitcoin SV, eCash, Navio. CashAddr encoder
  (hand-rolled BCH polymod) for BCH + XEC.
- New crate `hodl-chain-ethereum`: ureq JSON-RPC 2.0 client with a swappable
  `JsonRpcTransport` trait, hand-rolled RLP encoder, EIP-1559 (type-0x02) tx
  build + EIP-155 sign + broadcast. EIP-55 address checksum encode + parse.
  BIP-44 `m/44'/60'/account'/0/index`. `NetworkParams::ETHEREUM_MAINNET` and
  `BSC_MAINNET` (eip155 chain id 56) — BSC reuses the same crate via BEP-44.
- New crate `hodl-chain-monero`: Ledger-compatible BIP-39 → spend/view
  derivation per PLAN
  (`spend = sc_reduce32(keccak256(bip32_at(m/44'/128'/0'/0/0)))`,
  `view = sc_reduce32(keccak256(spend))`) — matches Cake Wallet, Monerujo
  (Ledger seed), Ledger Live; does NOT match monero-wallet-cli / GUI / MyMonero.
  LWS (open-monero-server) client for view-key sync, daemon JSON-RPC for
  `sendrawtransaction`.

#### Core types + config

- `hodl-core` types: `ChainId`, `Address`, `Amount`, `FeeRate`, `TxId`, `TxRef`,
  `SendParams`, `UnsignedTx`, `SignedTx`, `PrivateKeyBytes` (Zeroize), and the
  `Chain` trait per `PLAN.md`. `Chain::derive_private_key` defaults to `Err`;
  chain crates that need it (Bitcoin) override.
- `hodl-config` endpoint registry: per-chain `endpoints` (Electrum / JSON-RPC /
  LWS), `tor`, `lock.idle_timeout_secs`, `kdf` preset. Loader returns in-memory
  defaults on missing file — never auto-writes.
- `hodl-config` address book: separate `address_book.toml`, `AddressBook` /
  `Contact { label, address, chain, note }`, explicit-save semantics,
  missing-file → default.
- `hodl-core::proxy::parse_socks5_url` helper.

#### TUI surfaces

- M2 surfaces in `hodl-tui`: onboarding (create + restore), accounts table with
  chain switcher, receive (terminal QR via `qrcode` half- block encoder + OSC-52
  clipboard yank via `hjkl-clipboard`), settings (endpoint / Tor / KDF /
  lock-timeout). All driven by `hjkl-form` (Form-Normal / Form-Insert modal
  grammar inside every field) and `hjkl-picker` overlays.
- M3 send screen: recipient (bech32 P2WPKH validator) / amount / fee tier
  (Slow=12 / Normal=6 / Fast=2 blocks / Custom sat/vB) / submit. Result pane
  shows the broadcast TxId.
- M8 address book screen: `hjkl-picker` list, `a` add (four-field form), `d` Y/N
  delete confirm. `b` from Accounts opens it.
- M8 multi-wallet switcher: `w` on the lock screen opens an `hjkl-picker` over
  discovered vaults; selecting a wallet always re-locks before opening the new
  one.

#### Transport + Tor

- New Electrum methods: `blockchain.scripthash.listunspent`,
  `blockchain.transaction.broadcast`, `blockchain.transaction.get`.
- Tor SOCKS5 passthrough on every backend. Enabled via `tor.enabled = true` +
  `tor.socks5 = "socks5://127.0.0.1:9050"` in config. Wired in Electrum (TCP +
  TLS via `socks = "0.3"`), ETH/BSC JSON-RPC and Monero LWS + daemon RPC (via
  ureq's `socks-proxy` feature).
- `hodl-wallet::storage::list_wallets` enumerates `.vault` stems for the
  multi-wallet picker.

#### CLI

- `hodl init` now drops into the modal-form onboarding TUI instead of the
  line-prompted flow. New `hodl restore` subcommand fires the restore flow
  directly.

#### Workspace deps

- `hjkl-form = "0.3"` (with `crossterm` feature), `hjkl-ratatui = "0.3"`,
  `hjkl-picker = "0.3"`, `hjkl-clipboard = "0.4"`, `qrcode = "0.14"`,
  `bech32 = "0.11"`, `bs58 = "0.5"` (with `check`), `rustls = "0.23"`,
  `webpki-roots = "0.26"`, `k256 = "0.13"`, `tiny-keccak = "2"`,
  `curve25519-dalek = "4"`, `base58-monero = "2"`, `socks = "0.3"`. `ratatui`
  bumped 0.28 → 0.30 and `crossterm` 0.28 → 0.29 to align with `hjkl-ratatui`.

### Changed

- `hodl-tui::lock` now uses `hjkl-form` for password entry — same Form-Normal /
  Form-Insert grammar as the rest of the TUI. Password is masked on render and
  zeroized on every unlock attempt.
- `hodl_tui::lock::Outcome` gains `Unlocked(UnlockedWallet)` so the lock screen
  hands the unlocked wallet up to the app loop instead of rendering an M1
  placeholder. The placeholder unlocked screen and manual `l` lock toggle are
  gone.
- Lock auto-timeout reads `Config.lock.idle_timeout_secs` at startup;
  `DEFAULT_IDLE_TIMEOUT` (5 min) is now a fallback only used when config load
  fails or the value is 0.
- Account screen: `s` opens Send for the focused address; settings rebinds to
  `S`; `b` opens the address book.
- `Chain` trait gains `derive_private_key(seed, account, change, index)` with
  default `Err` impl. `BitcoinChain` overrides.

### Deferred (post-v1 / future work)

- Legacy P2PKH and wrapped-segwit (P2SH-P2WPKH) input signing.
- BCH / XEC send (sighash variants not implemented).
- RBF / fee bumping.
- Multi-source-address UTXO aggregation (send uses one source address).
- Monero ring signatures, bulletproofs, stealth addresses, subaddresses, native
  25-word seed import. xNAV blsCT shielded spends for Navio.
- ETH ERC-20 / BEP-20 token support, history (needs an external indexer).
- Real-network integration tests (all transport is mocked).
- Send-screen address-book picker integration.
- Price feed, packaging (deb / brew / scoop).

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

[Unreleased]: https://github.com/kryptic-sh/hodl/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/kryptic-sh/hodl/releases/tag/v0.3.0
[0.2.0]: https://github.com/kryptic-sh/hodl/releases/tag/v0.2.0
[0.1.2]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.2
[0.1.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.1
[0.1.0]: https://github.com/kryptic-sh/hodl/releases/tag/v0.1.0
[0.0.2]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.2
[0.0.1]: https://github.com/kryptic-sh/hodl/releases/tag/v0.0.1
