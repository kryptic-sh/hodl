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

| Chain           | Symbol | SLIP-44 | Light protocol                         | Notes                                                           |
| --------------- | ------ | ------- | -------------------------------------- | --------------------------------------------------------------- |
| Bitcoin         | BTC    | 0       | Electrum protocol (1.4) / BIP-157/158  | Reference impl. P2PKH + P2SH + bech32 + taproot.                |
| Ethereum        | ETH    | 60      | JSON-RPC (Infura / Alchemy / Ankr / …) | EIP-155 chain id. EIP-1559 fee market.                          |
| BNB Smart Chain | BNB    | 60      | JSON-RPC, EVM-compatible               | Reuses ETH derivation + signer. Chain id 56.                    |
| Monero          | XMR    | 128     | LWS (light-wallet server) protocol     | View-key sync via remote node. Send via own node API.           |
| Navio           | NAVIO  | 130     | ElectrumX (Electrum-Navio servers)     | Bitcoin-derivative codebase, PoS. xNAV privacy token via blsCT. |

### Tier 2 — Bitcoin-derivative chains, by market cap

| Chain        | Symbol | SLIP-44 | Notes                                                           |
| ------------ | ------ | ------- | --------------------------------------------------------------- |
| Litecoin     | LTC    | 2       | Code-fork. Electrum-LTC servers. Native MWEB optional, post-v1. |
| Dogecoin     | DOGE   | 3       | Litecoin descendant. Electrum-Doge servers or public RPC.       |
| Bitcoin Cash | BCH    | 145     | Chain-fork of BTC. CashAddr encoding. Electrum-Cash protocol.   |
| Bitcoin SV   | BSV    | 236     | Chain-fork of BCH. Electrum SV servers.                         |
| eCash        | XEC    | 1899    | Chain-fork of BCH. Chronik / Electrum-Cash-protocol fork.       |
| Bitcoin Gold | BTG    | 156     | Equihash PoW; key derivation identical to BTC. ElectrumG.       |

### Tier 3 — explicitly out of scope for v1

Bitcoin Diamond, DigiByte, Vertcoin, Zcash, Dash, and other forks. Add later if
demand exists; the chain abstraction should make the marginal cost low.

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
|   hodl-chain-bitcoin          |     BTC + BCH + BSV + LTC + DOGE + BTG + XEC
|   hodl-chain-ethereum         |     ETH + BSC (EVM)
|   hodl-chain-monero           |     XMR
+-------------------------------+
              |
              v
+-------------------------------+
| hodl-core                     |   shared types, errors, units, fee model
| hodl-config                   |   TOML loader, endpoint registry
+-------------------------------+
```

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
Litecoin / Doge / BCH / BSV / BTG / XEC after BTC works is mostly a config
record + endpoint list.

## Light-Wallet Backends

Per-chain endpoint lists ship as defaults; users can override or point at their
own node in `config.toml`.

| Family   | Protocol                 | Privacy notes                                                               |
| -------- | ------------------------ | --------------------------------------------------------------------------- |
| Bitcoin  | Electrum 1.4             | Server sees scripthashes you query → leaks address linkage. Tor optional.   |
| Bitcoin  | BIP-157/158 (Neutrino)   | Better privacy; client filters blocks itself. Heavier bandwidth. Post-v1.   |
| Ethereum | JSON-RPC                 | Provider sees address queries. Multi-provider rotation reduces signal.      |
| BSC      | JSON-RPC                 | Same as ETH. Default to public RPC, Ankr fallback.                          |
| Monero   | LWS (open-monero-server) | Server learns view key in plaintext under naive setup. Default = self-host. |

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

1. **Onboarding** — create new (12 / 24 word picker), restore from existing
   mnemonic, set passphrase, set vault password.
2. **Account list** — per-chain accounts, balances, last-sync timestamp.
3. **Receive** — address QR (terminal QR via `qrcode`), copy to clipboard,
   derivation path display.
4. **Send** — recipient, amount (with fiat preview from optional price feed),
   fee selector (slow / normal / fast / custom), confirmation, broadcast.
5. **History** — per-account tx list with confirmations, mempool state.
6. **Settings** — endpoint overrides, Tor toggle, KDF strength, lock timeout.
7. **Lock screen** — auto-locks after idle; vault password to unlock.

Keymap: vim-ish (`j/k` move, `gg/G` jump, `:` command, `/` search, `q` quit).

## Milestones

- **M0 — scaffold.** Workspace, crates, CI lint/build/test on Linux. ✅ done.
- **M1 — wallet core.** BIP-39 (12/24), BIP-32, encrypted vault, password
  unlock, lock-screen, Argon2id KDF, zeroize discipline. No chain support yet.
- **M2 — Bitcoin (mainnet+testnet).** Electrum client, address derivation
  (BIP-44/49/84/86), balance, history, gap-limit scan.
- **M3 — Bitcoin send.** PSBT build, fee estimation, sign, broadcast, RBF
  support.
- **M4 — Ethereum.** JSON-RPC client, EIP-155 sign, EIP-1559 fees, send native
  ETH. ERC-20 read post-v1.
- **M5 — BNB Smart Chain.** Reuses M4 crate; chain-id 56, default RPC list.
- **M6 — Bitcoin family.** BCH, LTC, DOGE, BSV, BTG, XEC. Each = constants
  record + endpoint list + address codec where it differs (CashAddr for BCH /
  XEC).
- **M7 — Monero.** BIP-39 → Ledger-compat derivation, view-key sync via LWS,
  send via own-node JSON-RPC.
- **M8 — polish.** Tor toggle, address book, multi-wallet switcher, optional
  price feed, packaged binaries (deb / brew tap / scoop).

Post-1.0 candidates: hardware-wallet bridge (Ledger / Trezor via HWI), Lightning
(LDK), ERC-20 / BEP-20 tokens, Polyseed for Monero, Neutrino for Bitcoin
privacy, taproot script-path spends.

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
