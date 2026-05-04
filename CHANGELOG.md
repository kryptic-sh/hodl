# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **TOFU cert pinning for Electrum TLS connections.** The previous
  `AcceptAnyServerCert` verifier (which accepted any TLS cert silently) is
  replaced with trust-on-first-use (TOFU) pinning, matching the default
  behaviour of Electrum desktop and Sparrow. On the first TLS connection to a
  given `host:port`, the SHA-256 fingerprint of the server's leaf certificate is
  recorded in `<data_root>/known_hosts.toml`. Subsequent connections to the same
  endpoint verify the fingerprint matches the saved value. A mismatch is treated
  as a **fatal, non-retryable security signal** (`Error::TofuMismatch`): the
  scan fails immediately with a message naming both the pinned and presented
  fingerprints, and the status line shows
  `scan failed: <chain>: connect: TOFU mismatch for <host:port> …`. The retry
  loop in the scan worker does **not** attempt a different endpoint on a TOFU
  mismatch — that would silently hide a potential MitM. CA bundle validation is
  intentionally omitted: Electrum servers overwhelmingly use self-signed certs
  that would be rejected by any CA bundle. TOFU provides the correct wallet
  trust model. New `hodl-config::KnownHosts` type manages the persistent pin
  store with atomic (temp-file + rename) saves. The `known_hosts.toml` file is
  never written on first load if it does not exist — it is created only when a
  new pin is established. `connect_tls` and `connect_tls_via_socks5` now return
  `(ElectrumClient, Option<String>)` — `Some(fp)` signals a new pin was
  established (caller must save). The `known_hosts` store is loaded once at
  `App::new_*` and shared via `Arc<Mutex<KnownHosts>>` across all scan threads
  and the send pipeline.

  **Recovering from a mismatch:** verify whether the server operator rotated
  their TLS certificate intentionally (e.g. cert renewal). If the rotation is
  legitimate, remove the stale entry from `known_hosts.toml` and reconnect — the
  new fingerprint will be pinned automatically. If you did not expect a cert
  change, investigate before reconnecting (potential MitM).

- **Streaming wallet scan with live summary updates.** The Accounts summary card
  now updates incrementally as each used address is discovered, instead of
  showing only a spinner for the full scan duration.
  `BitcoinChain::scan_used_addresses_streaming` adds a
  `&mut dyn FnMut(&UsedAddress)` callback fired on every used address before
  continuing the gap walk; `scan_used_addresses` becomes a thin no-op wrapper.
  The TUI background worker sends `ScanEvent::Used` per discovery and
  `ScanEvent::Done` on completion; `AccountState` accumulates a `partial_scan`
  from `Used` events so the card shows live confirmed/pending/total balances and
  running used-address count (with the spinner frame inline) throughout the
  scan. The status line reads `<chain> · scanning N used so far ⠋` during the
  walk. The Addresses sub-view (`d`) remains gated on scan completion — it opens
  a post-scan snapshot, not the partial accumulator.

### Added

- **Encrypted on-disk scan cache + `R` resync keybind.** Scan results
  (`WalletScan`) are now persisted per-wallet, per-chain at
  `<data_root>/cache/<wallet_name>/<ticker>.cache`. Each file is a
  ChaCha20-Poly1305 blob whose key is derived as
  `SHA-256("hodl-cache-v1\0" || seed)` — the seed is already 512 bits of
  high-entropy material from BIP-39 PBKDF2-HMAC-SHA512, so no Argon2 cost is
  paid (the unlock step paid it once for the seed). New `hodl_wallet::cache`
  module ships generic `encrypt` / `decrypt` / `derive_cache_key` helpers;
  `UnlockedWallet::cache_key()` wraps the derivation. New
  `hodl_tui::scan_cache::ScanCache` manages an in-memory
  `HashMap<ChainId, Arc<WalletScan>>` hydrated from disk on unlock and
  zeroized + dropped on lock (cache key included). Lookups are O(1) on the UI
  thread; writes go through atomic temp-file + rename. On unlock the Accounts
  summary card is now primed instantly from the cached snapshot while a
  background resync runs transparently — the status line reads
  `<chain> · refreshing  ⠋` instead of the cold-scan
  `<chain> · scanning N used so far`. New `R` (Shift+r) keybind on the Accounts
  screen forces a full from-scratch scan ignoring the cache; the freshly
  completed scan overwrites the on-disk blob via `ScanCache::put` after
  `ScanEvent::Done`. `BalanceSplit`, `UsedAddress`, and `WalletScan` now derive
  `Serialize` / `Deserialize` so they can be round-tripped through TOML inside
  the encrypted blob.
- **Persistent debug log at `<data_root>/hodl.log`.** All `tracing` events are
  appended to a sync-written, no-ANSI log file under the data directory so
  post-crash review is possible even after the alt-screen tears down. Default
  filter is `info,hodl*=debug` (override with `RUST_LOG`). When stdout is
  **not** a TTY (piped/CI), events are also tee'd to stderr; when stdout is a
  TTY the TUI owns the terminal and the file is the only sink (extra stderr
  writes would corrupt the alt-screen frame). Log file is opened for append, so
  logs accumulate across runs.
- **Addresses sub-view (`d` from Accounts screen).** Pressing `d` on the
  Accounts screen (once a scan is complete and at least one used address exists)
  opens a new read-only `Addresses` screen backed by the cached `WalletScan` —
  no network round-trip. The table shows every used address with its derivation
  path, type (`recv` / `chg`), confirmed balance, and pending balance. `j`/`k`
  navigate the list; `g`/`G` jump to first/last; `q`/`Esc` returns to the
  Accounts screen restoring the cached scan; `?` opens the help overlay. Mouse
  scroll moves the selection one row per event. The `AccountState` is stashed on
  `App` while the Addresses screen is open and restored on close so no re-scan
  is triggered. `ActiveChain::path_with_change(account, change, index)` added to
  produce change-aware BIP-44 path strings; the existing `derivation_path` now
  delegates to it with `change=0`.

- **`hodl-chain-bitcoin`: wallet-scan data layer with BIP-44 gap-limit walker.**
  New public API in `hodl-chain-bitcoin`:
  - `BalanceSplit` — confirmed/pending balance pair in sats with `total()` and
    `is_zero()` helpers.
  - `UsedAddress` — a single address discovered during a gap-limit scan (index,
    change chain, address string, `BalanceSplit`).
  - `WalletScan` — result of a full two-chain scan: `used: Vec<UsedAddress>`,
    `total: BalanceSplit`, and per-chain highest-index diagnostics.
  - `BitcoinChain::scan_used_addresses(seed, account, gap_limit)` — walks both
    the receive (change=0) and change (change=1) chains, querying Electrum for
    history count and balance at each derived address, stopping each chain after
    `gap_limit` consecutive unused addresses. Returns `WalletScan`.
  - `ElectrumClient::get_history_count(scripthash)` — thin wrapper over
    `scripthash_get_history` returning just the entry count; used by the scanner
    to avoid allocating the full history vector per address.
  - `ElectrumClient::new_unconnected()` — panicking null-transport client for
    derivation-only use-cases (no Electrum dial).

### Changed

- **Accounts screen redesigned as a per-chain summary card.** The previous 5-row
  address table is replaced by a centred summary card showing confirmed balance,
  pending balance, total balance, used-address count, and gap limit for the
  selected chain. While the background scan runs, the card body shows
  `scanning… ⠋` in cyan and the status line reads `<chain> · scanning…`. After
  the scan completes the status shows `<chain> · synced N/N used` in green; on
  failure it shows `<chain> · scan failed: <err>` in red. The `d` keybind now
  triggers `OpenAddresses` (routing wired in Step C). Mouse-scroll on the
  accounts table removed (no table). `j`/`k` keybinds removed. `S` (settings) is
  no longer blocked while scanning. Receive address is picked as the first used
  receive-chain address from the scan, falling back to derived index 0.

- **Endpoint selection is randomised with automatic fail-over.**
  `ActiveChain::from_chain_id` now shuffles the configured endpoint list per
  `ChainId` and tries each candidate in turn until one connects. Failed servers
  are skipped for the remainder of that connection attempt so a dead endpoint
  can't block a working one further down the list. Applies to Electrum (BTC
  family), JsonRpc (EVM), and LWS (Monero).

- **Spinner extracted into reusable `Spinner` widget** (`hodl-tui::spinner`) —
  `SPINNER_FRAMES`, `tick()`, `current()`, and `draw()` are now shared across
  all screens. `lock.rs` migrated from inline `spinner_frame: usize` + literal
  frame array to `Spinner`; behaviour is identical.

- **Account loading is now off-thread** — `start_load` spawns a background
  thread that opens the Electrum/RPC connection, derives 5 addresses, and
  queries balances. The event loop polls `pending_load.try_recv()` at 80 ms
  intervals while loading; the animated `loading accounts… ⠋` spinner replaces
  the former static placeholder. Navigation keys that require loaded rows
  (`r`/`s`/`b`/`S`/`p`) are suppressed during load; `q` and Ctrl-C/D always
  work. Seed bytes are zeroized in the thread before exit. Chain-switch and
  screen-return paths (`ChainSwitched`, address book, receive, send, settings)
  all call `start_load` instead of the old synchronous `load_accounts`.

- **Send build + broadcast are now off-thread** — submitting the Send form
  transitions to `Phase::Building` (spawns a thread for `estimate_fee` +
  `build_send`), then `Phase::Broadcasting` (spawns a thread for
  `sign_and_broadcast`). Each phase shows an animated `building… ⠋` /
  `broadcasting… ⠋` spinner at 80 ms cadence. Tab/Enter are blocked during both
  phases to prevent double-submit; Ctrl-C/D still quits. Seed bytes are zeroized
  in both threads before exit.

### Fixed

- **NavCoin generates legacy P2PKH (`N…`) addresses, not bech32.** The previous
  default of Bip84 produced `nav1q…` addresses that were unspendable:
  navcoin-core (verified against 7.0.3 and master 2026-05-04) has no
  `Bech32HRP()` method on `CChainParams` and no `bech32_hrp` field —
  bech32/segwit is unimplemented in upstream. `default_send_purpose(NavCoin)`
  now returns `Bip44`; `validate_purpose` rejects Bip49/84/86 for NAV; the TUI
  recipient validator accepts only legacy P2PKH base58check (`N…`) for NavCoin;
  placeholder text changed from `nav1q…` to `N…`. Existing wallets that derived
  `nav1q…` addresses should re-derive — no funds were ever spendable to those
  addresses.

- **Vault unlock no longer freezes the TUI** — argon2id KDF runs on a background
  thread; the lock screen shows a `decrypting… ⠋` spinner during the ~1–2 s wait
  instead of hanging with no feedback.
- **Mouse wheel now scrolls one row per tick** on the accounts table and address
  book list — previously mouse capture was disabled, so the terminal emulator
  translated wheel ticks into repeated arrow-key sequences whose speed varied
  across terminals and could jump multiple rows per click. Mouse capture is now
  enabled (`EnableMouseCapture` / `DisableMouseCapture`) and each
  `ScrollUp`/`ScrollDown` event maps to exactly one `move_selection(±1)` call.

### Added

- **Contextual help overlay (`?` / `F1`) per screen** — every TUI screen now
  exposes a `help_lines() -> Vec<(String, String)>` method listing its actual
  keybindings. Pressing `?` (or `F1` on form-input screens where `?` can be
  typed) opens a centred, scrollable two-column overlay (`key | description`)
  drawn on top of the active screen. `Esc`, `q`, or `?` closes it; `j`/`k` /
  `↓`/`↑` scroll; `g`/`Home` jumps to top; `G`/`End` to bottom. Implemented in
  new module `hodl-tui::help` (`HelpOverlay`, `HelpAction`). Lock, Accounts,
  AddressBook, and Receive screens use `?`; Send and Onboarding use `F1` so `?`
  remains typeable in form fields. Help lines are modal-aware: Send returns
  form-edit binds in Insert mode and navigation binds otherwise; Onboarding
  returns Confirm-pane binds during the mnemonic-confirmation gate.

- **`cargo-deny` in CI** — strict gate on advisories, licenses, bans
  (multiple-versions, wildcards), and sources. Workspace `toml` bumped from
  `0.8` to `1` to match `hjkl-config` and collapse the
  `winnow`/`toml_datetime`/`serde_spanned` duplicate-dep cluster.
  Intra-workspace path deps now carry an explicit `version = "0.3"` so they no
  longer trigger the wildcard-deny rule.

- **Bip49 (P2SH-P2WPKH) signing for LTC/NAV wrapped-segwit addresses** in
  `hodl-chain-bitcoin`. New helpers: `p2sh_p2wpkh_redeem_script`, `p2sh_script`,
  `sign_inputs_p2sh_p2wpkh`, `p2sh_scripthash`. Each input gets
  `scriptSig = pushdata(redeemScript)` and a 2-item witness stack
  `[sig||hashtype, pubkey]`. `sign_multi_source` dispatches to the new signer
  for `Purpose::Bip49`; `decode_address_to_script` decodes P2SH base58check
  addresses and validates the `p2sh_prefix` version byte; `scripthash_for`
  distinguishes P2SH from P2PKH by version byte. `compute_sighash` no longer
  errors on `Bip49` — uses BIP-143 sighash (same scriptCode as native P2WPKH).

- **Legacy P2PKH signing** in `hodl-chain-bitcoin`: pre-segwit sighash for
  BTC-family chains (DOGE, NAV-via-Bip44, LTC-via-Bip44) plus BCH's
  BIP-143-shaped FORKID sighash (SIGHASH_ALL | SIGHASH_FORKID = 0x41). New
  helpers: `legacy_p2pkh_sighash`, `bch_sighash`, `compute_sighash` dispatcher,
  `p2pkh_script_sig`, `p2pkh_script`, legacy tx serialization
  (`serialize_unsigned_tx_legacy`, `serialize_signed_tx_legacy`) with no segwit
  marker/flag and a scriptSig per input. `sign_inputs_legacy_p2pkh` orchestrates
  per-input signing into a final non-segwit transaction.
- `BitcoinChain::default_send_purpose(chain_id)` picks the right derivation
  purpose per chain: `Bip44` for DOGE and BCH (segwit not deployed); `Bip84` for
  everything else in the BTC family.
- `cashaddr::decode_p2pkh_cashaddr` — decode a CashAddr P2PKH address to its
  20-byte pubkey hash (with checksum verification). Needed for BCH UTXO lookup
  and recipient script construction.
- `electrum::p2pkh_scripthash` — Electrum scripthash for a P2PKH scriptPubKey.
  Enables UTXO queries for legacy (base58check and CashAddr) addresses.
- `BitcoinChain::scripthash_for` now handles bech32 P2WPKH, CashAddr P2PKH, and
  legacy base58check P2PKH — all three address families used across the
  BTC-chain family.
- LTC / DOGE / BCH / NAV send dispatch in `active_chain.rs` is no longer gated —
  every BTC-family chain has a working `build_send` / `sign_multi_source` path.

### Fixed

- **Wrong-chain address protection in `decode_address_to_script`** — the signer
  now validates legacy P2PKH addresses against the active chain's `p2pkh_prefix`
  byte (and CashAddr against the chain's HRP). Previously, sending DOGE to a BTC
  `1…` address would silently encode the wrong scriptPubKey and burn the funds.
  Now returns `Error::Codec` naming the expected chain.
- **TUI recipient validator accepts legacy + CashAddr addresses** for the chains
  that need them. Bitcoin / LTC / NAV / BTC-testnet accept bech32 segwit-v0
  (preferred) or legacy P2PKH base58check; DOGE accepts only legacy P2PKH; BCH
  accepts only CashAddr. The previous validator demanded bech32 for the entire
  BTC family, which silently blocked send for DOGE/BCH/legacy-BTC even though
  the chain crate supported them.
- **Multi-source UTXO**: Bitcoin send now aggregates UTXOs across every funded
  address in the gap-scan, coin-selecting across the merged pool. Closes the
  v0.2.0 deferred item where wallets with funds spread over multiple derived
  addresses hit "insufficient funds" with enough total balance. New API:
  `BitcoinChain::build_tx_multi_source` + `sign_multi_source` with `InputHint`
  per input.
- **RBF (BIP-125)**: opt-in checkbox on the send screen. When enabled, every
  input's sequence is `0xfffffffd` (RBF-signaling); otherwise `0xffffffff`
  (final). Default is off — explicit user choice.
- **Chain-aware TUI dispatch**: new `hodl_tui::active_chain::ActiveChain` enum
  (Bitcoin / Ethereum / Monero) wraps the per-chain crates with a uniform
  `derive` / `balance` / `build_send` / `sign_and_broadcast` surface. Factory
  `from_chain_id(id, config)` picks the right `NetworkParams` + endpoint type
  based on the user's config and Tor toggle. Account screen now re-derives rows
  when the chain picker flips selection — the picker is no longer decorative.
- ETH send wired end-to-end through the TUI. Recipient validator switches to
  EIP-55 for Ethereum / BSC.

### Changed

- `BitcoinChain::new` defaults `purpose` per-chain via `default_send_purpose`
  instead of always `Bip84`. Override via `with_purpose` if you need a
  non-default path (e.g. Bip44 on LTC for legacy addresses).
- Send TUI passes `(account, total_balance)` to the chain instead of a single
  `(address, index)` pair. Account screen rebinds Send accordingly.
- `account::AccountAction::OpenSend` carries `chain: ChainId` so the Send screen
  builds the right chain. `send.rs` no longer hardcodes `BITCOIN_MAINNET`. The
  3× `BitcoinChain::new` reconnect dance in `try_submit` is gone — `ActiveChain`
  owns the connection.
- Recipient validation is per-chain (`validate_recipient(s, chain_id)`). Bitcoin
  family: bech32 segwit v0. Ethereum / BSC: EIP-55 hex. Monero: gated "not
  implemented".

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
