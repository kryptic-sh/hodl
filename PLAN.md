# hodl — Plan

Living roadmap. Scope, phases, deliverables. Update as work lands.

## Vision

Light crypto wallet for the terminal. One binary, ratatui UI, BIP-39 seed
restores every supported chain. Local keys, encrypted at rest. Talks to public
or self-hosted light-wallet endpoints — never runs a full node, never phones
home.

## Non-Goals

- Not a full node for any chain. No UTXO sync from genesis, no chain validation.
- Not a custodian. No hosted keys, no remote signer.
- Not a DEX, not a DeFi front-end, not a portfolio tracker. Send / receive /
  balance only in v1.
- No smart-contract authoring, no token launching.
- No mobile, no GUI, no browser extension.

## Target Platforms

| Platform | Tier | Notes                                                       |
| -------- | ---- | ----------------------------------------------------------- |
| Linux    | 1    | Primary dev target. Static musl build for portable release. |
| macOS    | 1    | Both arm64 and x86_64. Notarized binary.                    |
| Windows  | 2    | Native console + Windows Terminal. MSI installer.           |

Terminals: any with truecolor + UTF-8. Falls back to 256-color on dumb
terminals.

## Supported Chains

Ordered by implementation priority. Priority weighs market cap, ecosystem
overlap with already-implemented code, and light-wallet protocol availability.

### Tier 1 — first cut

| Chain           | Symbol | SLIP-44 | Light protocol                         | Notes                                                            |
| --------------- | ------ | ------- | -------------------------------------- | ---------------------------------------------------------------- |
| Bitcoin         | BTC    | 0       | Electrum protocol (1.4) / BIP-157/158  | Reference impl. P2PKH + P2SH + bech32 + taproot.                 |
| Ethereum        | ETH    | 60      | JSON-RPC (Infura / Alchemy / Ankr / …) | EIP-155 chain id. EIP-1559 fee market.                           |
| BNB Smart Chain | BNB    | 60      | JSON-RPC, EVM-compatible               | Reuses ETH derivation + signer. Chain id 56.                     |
| Monero          | XMR    | 128     | LWS (light-wallet server) protocol     | View-key sync via remote node. Send via own node API.            |
| NavCoin         | NAV    | 130     | ElectrumX (Electrum-NavCoin servers)   | Bitcoin-derivative, PoS. xNAV privacy token via blsCT (post-v1). |

### Tier 2 — Bitcoin-derivative chains, by market cap

| Chain        | Symbol | SLIP-44 | Notes                                                           |
| ------------ | ------ | ------- | --------------------------------------------------------------- |
| Litecoin     | LTC    | 2       | Code-fork. Electrum-LTC servers. Native MWEB optional, post-v1. |
| Dogecoin     | DOGE   | 3       | Litecoin descendant. Electrum-Doge servers or public RPC.       |
| Bitcoin Cash | BCH    | 145     | Chain-fork of BTC. CashAddr encoding. Electrum-Cash protocol.   |

### Tier 3 — explicitly out of scope for v1

Bitcoin Gold (51%-attacked twice, near-dead dev activity), Bitcoin Diamond,
DigiByte, Vertcoin, Zcash, Dash, **Bitcoin SV** (CMC #113, ~$320M cap, no real
ecosystem), **eCash** (CMC #167, ~$145M cap, near-dead post-rebrand), and other
forks. Add later if demand exists; the chain abstraction should make the
marginal cost low.

## Mnemonic & Key Derivation

- **Standard:** BIP-39. Accept and generate **12-word (128-bit entropy)** and
  **24-word (256-bit entropy)** seeds. Other lengths rejected on input.
- **Optional passphrase:** BIP-39 25th word ("seed extension"). Treated as part
  of the seed; wrong passphrase silently produces a different wallet — flag this
  in onboarding copy.
- **Seed → master key:** BIP-39 PBKDF2-HMAC-SHA512 → 64-byte seed → BIP-32
  master.
- **Account paths:** BIP-44
  `m / 44' / coin_type' / account' / change / address_index`. BIP-49
  (P2SH-segwit) and BIP-84 (native segwit) for Bitcoin and forks that support
  them. BIP-86 (taproot) for BTC.
- **Address discovery:** BIP-44 gap-limit scan (default 20, configurable per
  chain).

### Monero from BIP-39 — known sharp edge

Monero's native seed format is the 25-word Electrum-style mnemonic (or 16-word
Polyseed). Restoring Monero from a BIP-39 seed requires a **non-standard
derivation**. We'll use the Ledger-compatible scheme:

```
spend_key = keccak256(BIP-32 derive at m/44'/128'/0'/0/0).reduce_to_scalar()
view_key  = keccak256(spend_key).reduce_to_scalar()
```

This matches Cake Wallet, Monerujo (with Ledger seed), and Ledger Live. Surface
a clear warning at restore time: "Your Monero address from this seed will only
match wallets that use the Ledger-compatible BIP-39 derivation."

Native 25-word Monero seeds may be supported as a separate import path post-v1.

## Architecture

```
+-------------------------------+
|         apps/hodl             |   main binary, CLI parse, TUI lifecycle
+-------------------------------+
              |
              v
+-------------------------------+
| hodl-tui (ratatui screens)    |   onboarding, accounts, send, receive, settings
|   uses hjkl-form (modal       |   vim-modal text fields (password, amount,
|        text fields)           |   recipient, mnemonic, passphrase)
|   uses hjkl-picker (fuzzy)    |   wallet / account / chain / endpoint pickers
|   uses hjkl-clipboard         |   yank address (OSC 52 for SSH), paste recipient
|   uses hjkl-ratatui           |   Style intern + KeyEvent bridge
+-------------------------------+
              |
              v
+-------------------------------+
| hodl-wallet                   |   BIP-39, BIP-32/44/49/84/86, vault, signing
+-------------------------------+
              |
              v
+-------------------------------+
| hodl-chain-* (per-family)     |   trait Chain — balance, history, send
|   hodl-chain-bitcoin          |     BTC + BCH + LTC + DOGE + NAV
|   hodl-chain-ethereum         |     ETH + BSC (EVM)
|   hodl-chain-monero           |     XMR
+-------------------------------+
              |
              v
+-------------------------------+
| hodl-core                     |   shared types, errors, units, fee model
| hodl-config (uses hjkl-config)|   XDG path resolution + TOML endpoint registry
+-------------------------------+
```

### hjkl crate adoption

The hjkl modal-editor stack ships several reusable crates we already half-adopt;
pulling more of them eliminates parallel implementations and gives `hodl` the
same vim-modal feel as `sqeel` and `buffr`.

| Crate            | Pin   | Adopted in | Replaces / enables                                                                                                                                                                                                                                                                              |
| ---------------- | ----- | ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `hjkl-config`    | `0.2` | M1 (done)  | XDG `data_dir("hodl")` for vault path. Extend in M2 for endpoint registry, lock timeout, Tor toggle.                                                                                                                                                                                            |
| `hjkl-form`      | `0.3` | M2         | Modal-vim forms (`Form-Normal` / `Form-Insert`). Each `TextFieldEditor` hosts a full `hjkl-engine::Editor`, so every field gets the full vim grammar (motions, operators, registers, counts). `SelectField` + `CheckboxField` + `SubmitField` + `Validator` cover the rest of the form surface. |
| `hjkl-picker`    | `0.3` | M2         | Wallet switcher, chain picker, account picker, address-book picker, endpoint picker.                                                                                                                                                                                                            |
| `hjkl-clipboard` | `0.4` | M2 receive | OSC 52 SSH-safe clipboard for "yank address"; paste recipient on send.                                                                                                                                                                                                                          |
| `hjkl-ratatui`   | `0.3` | M2         | `Style` intern + `KeyEvent` bridge so we share theming / input shape with sqeel and buffr.                                                                                                                                                                                                      |
| `hjkl-engine`    | —     | transitive | Pulled by `hjkl-form` / `hjkl-picker`; gives modal FSM under the text fields.                                                                                                                                                                                                                   |
| `hjkl-buffer`    | —     | transitive | Rope buffer underneath the form fields.                                                                                                                                                                                                                                                         |
| `hjkl-bonsai`    | —     | N/A        | Tree-sitter syntax; no use in a wallet TUI.                                                                                                                                                                                                                                                     |
| `hjkl-editor`    | —     | N/A        | Full modal editor; overkill for fixed-shape wallet forms.                                                                                                                                                                                                                                       |

All hjkl crates are consumed from **crates.io** at the pins above — no
`[patch.crates-io]` block, no git submodules. `hodl` tracks released hjkl
versions only; if the wallet needs an upstream change, land it in the relevant
`hjkl-*` repo, cut a release, then bump the pin here. Mirrors how `sqeel-tui`
consumes `hjkl-form` / `hjkl-picker` / `hjkl-clipboard` / `hjkl-ratatui` and how
`sqeel-core` consumes `hjkl-config`.

### `Chain` trait (sketch)

```rust
pub trait Chain {
    fn id(&self) -> ChainId;
    fn slip44(&self) -> u32;
    fn derive(&self, seed: &Seed, account: u32, index: u32) -> Result<Address>;
    fn balance(&self, addr: &Address) -> Result<Amount>;
    fn history(&self, addr: &Address) -> Result<Vec<TxRef>>;
    fn estimate_fee(&self, target_blocks: u32) -> Result<FeeRate>;
    fn build_tx(&self, params: SendParams) -> Result<UnsignedTx>;
    fn sign(&self, tx: UnsignedTx, key: &PrivateKey) -> Result<SignedTx>;
    fn broadcast(&self, tx: SignedTx) -> Result<TxId>;
}
```

The Bitcoin-family crate parameterizes a single implementation by network
constants (magic bytes, address HRP, dust limit, default port), so adding
Litecoin / Doge / BCH / NavCoin after BTC works is mostly a config record +
endpoint list. NavCoin's blsCT-based xNAV privacy spends are the one exception —
they need a dedicated module on top of the base UTXO codec (post-v1).

The newer **Navio** chain (Navio-project, distinct from legacy NavCoin) is
deferred — when its prefix bytes / HRP / xNAV variant stabilize we'll add a
second `NetworkParams` record alongside `NAVCOIN_MAINNET`.

## Light-Wallet Backends

Per-chain endpoint lists ship as defaults; users can override or point at their
own node in `config.toml`.

| Family   | Protocol                 | Privacy notes                                                                                  |
| -------- | ------------------------ | ---------------------------------------------------------------------------------------------- |
| Bitcoin  | Electrum 1.4             | Server sees scripthashes you query → leaks address linkage. Tor optional.                      |
| Bitcoin  | BIP-157/158 (Neutrino)   | Better privacy; client filters blocks itself. Heavier bandwidth. Post-v1.                      |
| Ethereum | JSON-RPC                 | Provider sees address queries. Multi-provider rotation reduces signal.                         |
| BSC      | JSON-RPC                 | Same as ETH. Default to public RPC, Ankr fallback.                                             |
| Monero   | LWS (open-monero-server) | Server learns view key in plaintext under naive setup. Default = self-host.                    |
| NavCoin  | ElectrumX                | Public spends like Bitcoin. xNAV (blsCT) spends shielded — server sees commitment, not amount. |

## Storage Layout

```
$XDG_DATA_HOME/hodl/
├── wallets/
│   └── <wallet-id>/
│       ├── vault.bin         # encrypted: argon2id → chacha20-poly1305(seed + meta)
│       └── cache.sqlite      # address → balance, tx history, last sync height
├── config.toml
└── log/
    └── hodl.log
```

- **Vault format:**
  `magic(8) | version(2) | argon2_params(16) | salt(16) | nonce(12) | ciphertext | tag(16)`.
  Plaintext is
  `bincode(Seed { mnemonic_id, passphrase_present, created_at, label })`.
- **KDF:** Argon2id, m=64MiB, t=3, p=1 by default; tunable up.
- **Cache** is purgeable; rebuilt from chain on demand.

## Security

- Seed and derived keys wrapped in `Zeroizing` / `secrecy::Secret` end-to-end.
- No plaintext seed touches disk, ever. No swap-file leakage — `mlock` the seed
  buffer on Linux/macOS where the rlimit allows.
- No telemetry. No update pings. No analytics. Networking only to configured
  RPC/Electrum endpoints.
- Address-book and transaction history is local-only.
- Tor support: `socks5://127.0.0.1:9050` proxy passthrough for all chain
  backends. Off by default; one-line config to enable globally.
- Reproducible release builds (`cargo auditable`, locked dependencies, signed
  tags).

## TUI Surfaces

### Modal-form principle

Every multi-field input in `hodl-tui` is a `hjkl_form::Form`. No bespoke input
loops, no parallel keymap layer. The form FSM gives us two modes that match
Vim's grammar exactly:

- **`Form-Normal`** — `j` / `k` (or `Tab` / `S-Tab`) move focus between fields;
  `i` / `a` enter the focused field's editor; `Enter` on a `SubmitField` runs
  the submit fn; `:` opens an ex line for global commands (`:w`, `:q`, `:lock`);
  `/` opens search where applicable.
- **`Form-Insert`** — keystrokes route to the focused field's
  `hjkl_engine::Editor`. Users get the **full vim grammar inside every text
  field**: `h j k l`, `w / b / e`, `0 / $`, `dw / ciw / x / r`, `u / C-r`,
  `y / p`, registers, counts. `Esc` returns to `Form-Normal`.

This is not "vim-ish" — it is the same modal editor that powers `hjkl-editor`
and `sqeel`'s query buffer. Users who already know vim get send-screen muscle
memory for free; users who don't can stay in insert mode and treat fields like
normal text inputs.

### Field-type mapping

| Field                    | hjkl-form type    | Notes                                                                                          |
| ------------------------ | ----------------- | ---------------------------------------------------------------------------------------------- |
| Vault password           | `TextFieldEditor` | Single-line, masked render; replaces M1's raw `String` buffer in `hodl-tui::lock`.             |
| BIP-39 mnemonic (import) | `TextFieldEditor` | Multi-line allowed; validator checks 12 / 24 word count + checksum before submit enables.      |
| BIP-39 passphrase        | `TextFieldEditor` | Single-line, masked.                                                                           |
| Send recipient           | `TextFieldEditor` | Validator: per-chain address codec check (bech32 / CashAddr / EIP-55 / Monero); paste via OSC. |
| Send amount              | `TextFieldEditor` | Validator: parses decimal in chain units, ≤ spendable balance.                                 |
| Send fee tier            | `SelectField`     | `slow / normal / fast / custom` — `custom` reveals a `TextFieldEditor` for sat/vB or gwei.     |
| Memo / label             | `TextFieldEditor` | Optional; per-tx note.                                                                         |
| Tor enable               | `CheckboxField`   | Toggles SOCKS5 passthrough on all chain backends.                                              |
| Confirm send             | `SubmitField`     | Submit fn = sign + broadcast; surfaces `SubmitOutcome` errors as field-level red text.         |
| Word-count picker        | `SelectField`     | 12 / 24 — onboarding only.                                                                     |
| KDF strength             | `SelectField`     | `default / hardened / paranoid` → preset Argon2id params.                                      |

Validators live in `hodl-tui` and use `hjkl_form::Validator`; submit buttons
stay disabled while any field validator returns an error, so we never hand a
half-valid `SendParams` to the chain layer.

### "Pick one of N" flows

`hjkl_picker::Picker` for fuzzy lists; all clipboard operations route through
`hjkl-clipboard` (OSC 52 fallback for SSH).

1. **Onboarding** — `hjkl-form` with the word-count `SelectField`, mnemonic /
   passphrase / password `TextFieldEditor`s, and a final `SubmitField` that
   creates the vault.
2. **Account list** — per-chain accounts, balances, last-sync timestamp. Wallet
   / account switch via `hjkl-picker`.
3. **Receive** — address QR (terminal QR via `qrcode`), yank to clipboard via
   `hjkl-clipboard` (OSC 52 over SSH), derivation path display.
4. **Send** — `hjkl-form` with recipient + amount `TextFieldEditor`s (paste
   recipient via `hjkl-clipboard`), fee `SelectField`, optional memo, confirm
   `SubmitField`. Validator gates the submit.
5. **History** — per-account tx list with confirmations, mempool state. Tx
   detail jump via `hjkl-picker`.
6. **Settings** — `hjkl-form` with endpoint `SelectField` (registry) +
   custom-URL `TextFieldEditor`, Tor `CheckboxField`, KDF `SelectField`,
   lock-timeout `TextFieldEditor`.
7. **Lock screen** — single-field `hjkl-form` (masked `TextFieldEditor` +
   implicit submit on `Enter` from `Form-Normal`). Auto-locks after idle.

Renderer: `hjkl_ratatui::form::draw_form` is the canonical adapter — no custom
widget code in `hodl-tui` for form chrome.

## Milestones

- **M0 — scaffold.** Workspace, crates, CI lint/build/test on Linux. ✅ done.
- **M1 — wallet core.** BIP-39 (12/24), BIP-32, encrypted vault, password
  unlock, lock-screen, Argon2id KDF, zeroize discipline. No chain support yet.
- **M2 — Bitcoin (mainnet+testnet).** Electrum client, address derivation
  (BIP-44/49/84/86), balance, history, gap-limit scan. **Also:** flip `hodl-tui`
  onto `hjkl-form` + `hjkl-picker` + `hjkl-ratatui` + `hjkl-clipboard`; retire
  the raw-`String` password buffer in `hodl-tui::lock`.
- **M3 — Bitcoin send.** PSBT build, fee estimation, sign, broadcast, RBF
  support.
- **M4 — Ethereum.** JSON-RPC client, EIP-155 sign, EIP-1559 fees, send native
  ETH. ERC-20 read post-v1.
- **M5 — BNB Smart Chain.** Reuses M4 crate; chain-id 56, default RPC list.
- **M6 — Bitcoin family.** BCH, LTC, DOGE. Each = constants record + endpoint
  list + address codec where it differs (CashAddr for BCH).
- **M7 — Monero.** BIP-39 → Ledger-compat derivation, view-key sync via LWS,
  send via own-node JSON-RPC.
- **M7.5 — NavCoin.** Public NAV via the Bitcoin-family path (constants record
  - Electrum-NavCoin endpoints). xNAV blsCT shielded spends as a follow-up
    module — receive only at first, full send post-v1. The newer **Navio** chain
    (separate project) lands later as a sibling `NetworkParams` record.
- **M8 — polish.** Tor toggle, address book, multi-wallet switcher, optional
  price feed, packaged binaries (deb / brew tap / scoop).

Post-1.0 candidates: hardware-wallet bridge (Ledger / Trezor via HWI), Lightning
(LDK), ERC-20 / BEP-20 tokens, Polyseed for Monero, Neutrino for Bitcoin
privacy, taproot script-path spends.

## Deferred

Items intentionally postponed during the v0.3.x cycle. Each carries a brief
rationale and any code that hints at the missing piece. Update as items land or
get re-prioritised.

### NavCoin Phase 2

- **xNAV blsCT shielded balance read.** Needs BLS12-381 dep (likely `blstrs` or
  `bls12_381`), a separate key derivation rooted in `SECRET_BLSCT_VIEW_KEY`
  (different from BIP-44), Pedersen commitment + range-proof decryption, and the
  NAV-specific Electrum method `blockchain.transaction.get_keys`. New crate
  `hodl-chain-navcoin` (don't pollute `hodl-chain-bitcoin`). xNAV outputs are
  invisible to the standard `blockchain.scripthash.*` path so a wallet with
  shielded funds will under-report total balance until this lands.
- **Cold-staking output visibility.** NavCoin cold-staking outputs use a custom
  scriptPubKey shape (`isColdStakingOutP2PKH`, `isColdStakingV2Out`) that does
  not match any standard P2PKH scripthash. navcoin-js queries
  `blockchain.staking.get_keys` to discover the (spending, staking) key pairs,
  builds a composite scripthash via `Script.fromAddresses(staking, spending)`,
  and subscribes via `blockchain.scripthash.subscribe`. hodl's BIP-44 gap scan
  will silently miss any funds the user has moved into cold staking. Spending
  cold-staking inputs is out of scope (we're not a staking node).
- **NAV add-on protocols.** NavNS (`blockchain.dotnav.resolve_name`), DAO
  (`blockchain.dao.subscribe`, `blockchain.consensus.subscribe`), NavToken
  (`blockchain.token.get_token`), NavNFT (`blockchain.token.get_nft`), outpoint
  subscribe (`blockchain.outpoint.subscribe`), staker votes
  (`blockchain.stakervote.subscribe`). All additive, none blocking; future
  features.
- **NAV fee-estimate fallback.** If `blockchain.estimatefee` returns ≤ 0, fall
  back to navcoin-js's hardcoded default of 100,000 satoshis/kB (0.001 NAV) to
  avoid zero-fee broadcast attempts. Defensive; not currently observed to fire.

### TUI / UX

- **Per-row spinner in Addresses sub-view.** Currently each row renders the
  cached value or a static dash. Now that the sub-view streams live, in-flight
  rows could show a spinner indicator next to balances that are still being
  fetched (vs. final values).
- **`format_atoms` decimals per chain.** The Addresses table's `format_atoms`
  assumes 8 decimals (BTC family). Wrong for ETH (18) and Monero (12); not yet
  visible because EVM/Monero scans degenerate to a single row, but will surface
  when those chains support multi-row display.
- **Send build/broadcast retry-on-failure.** The scan worker retries up to 3
  times on `Network`/`Io`/`Endpoint` errors via `try_endpoints` re-shuffle.
  `send.rs::build_thread` and `broadcast_thread` are still single-attempt — a
  network blip mid-broadcast bails. Apply the same pattern.
- **TOFU mismatch UX.** Today a fingerprint mismatch surfaces as a red
  status-line error pointing at `known_hosts.toml`. A guided remediation flow
  (show old vs new fingerprint, offer "trust new" / "stay pinned" / "abort"
  prompts) would be friendlier — and would let a user re-pin without editing
  TOML by hand.
- **Send-screen address book picker.** Wire `hjkl-picker` into the recipient
  field so the user can select from saved contacts instead of typing the
  address.

### Per-chain scan strategies

- **EVM multi-account scan.** `scan_thread::ActiveChain::Ethereum` currently
  derives a single address (account 0, index 0) and reports its balance. Could
  scan multiple accounts (`m/44'/60'/N'/0/0` for N=0..gap_limit) to surface
  multi-account wallets. Same for BSC.
- **Monero refresh via LWS.** `scan_thread::ActiveChain::Monero` is the same
  single-derive degenerate path; should hit `lws::get_address_info` /
  `get_unspent_outs` for proper subaddress account discovery.
- **ERC-20 token reads.** Original v0.4.0 candidate. `eth_call` against ERC-20
  contracts to surface stablecoin/token balances on the Ethereum summary card.

### Hardening / hygiene

- **Bip49 wrapped-segwit signing for LTC** is shipped; the `Bip49 → p2wpkh`
  fallback in `purpose_script` is now reachable, so the dead-code marker is gone
  — keep an eye on this if more chains add Bip49 support.
- **Real-network smoke CI.** The curated default Electrum servers are manually
  probed on demand; add a CI workflow (or scheduled job) that does a smoke
  connect + `server.version` against each, opening an issue when one goes down.

## Open Questions

- **Price feed.** Single source (CoinGecko) or none? Adds a network call per
  refresh; opt-in by default to preserve no-phone-home property.
- **Default Electrum servers.** Curate own list vs. consume the public peer
  discovery? Curated is more predictable, less private.
- **Vault password vs. OS keyring.** Keyring is friction-free but adds platform
  surface; password-only is portable. Start with password; revisit.
- **Monero light-wallet trust model.** Default to "you must run your own LWS"
  vs. ship a default endpoint? Defaulting harms privacy; not defaulting harms
  UX. Likely: ship onboarding wizard that nudges self-host but allows public
  during setup, with red-letter warning.
