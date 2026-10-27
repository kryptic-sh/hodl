# Backlog

## Dependencies

- **Two RustCrypto generations are in the tree.** `k256` 0.14 and `bip32` 0.6
  (moved on 2026-10-27 to clear RUSTSEC-2026-0387 in `crypto-bigint` 0.5) are on
  `digest` 0.11 / `rand_core` 0.10, while other dependencies are still on
  `digest` 0.10 / `rand_core` 0.6, so `deny.toml` skips the old generation's
  crates. Each has a release on the new generation except where noted:
  - `argon2` 0.5 → 0.6 (with `password-hash` 0.6, `blake2` 0.11): the wallet
    file's KDF. Before moving, confirm 0.6 derives the same key from the same
    parameters, or existing wallets stop opening; a test that opens a wallet
    file written by 0.5 would pin it.
  - `chacha20poly1305` 0.10 → 0.11 (`aead` 0.6, `cipher` 0.5): the wallet file's
    cipher. Same check: an existing file must still decrypt.
  - `curve25519-dalek` 4 → 5: Monero.
  - `bls12_381` 0.8 → 0.9: Navio. Moves `ff` / `group` to 0.14, but 0.9 is still
    on `digest` 0.10.
  - `rand` 0.8 → 0.10 (dependabot #30, red on its own).
  - No release yet: `bs58` 0.5.1 (its latest) uses `sha2` 0.10, `bip39` 3 caps
    `rand_core` below 0.7, `ring` uses `getrandom` 0.2, and `hkdf` 0.12 uses
    `hmac` 0.12.

  Drop each `skip` in `deny.toml` once nothing pulls that version.
