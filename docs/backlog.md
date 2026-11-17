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
    cipher. Same check: an existing file must still decrypt. It also drops the
    `chacha20` 0.9 skip, beside `rand` 0.10's `chacha20` 0.10.
  - `curve25519-dalek` 4 → 5: Monero.
  - `bls12_381` 0.8 → 0.9: Navio. Moves `ff` / `group` to 0.14, but 0.9 is still
    on `digest` 0.10.
  - No release yet: `bs58` 0.5.1 (its latest) uses `sha2` 0.10, `ring` uses
    `getrandom` 0.2, and `hkdf` 0.12 uses `hmac` 0.12.

  Drop each `skip` in `deny.toml` once nothing pulls that version.

- **hodl reads the terminal through crossterm; hjkl 0.46 uses ttyr.** The
  `hodl_tui::keys` module converts crossterm keys to hjkl's `Input` and drives
  pickers itself, mirroring what `hjkl-engine-tui` and `hjkl-picker-tui` did in
  0.43. Moving hodl's event loop and backend to ttyr would let it use hjkl's
  `ttyr_to_input` and `hjkl_picker_tui::handle_key` again and drop the
  `signal-hook@0.3` skip in `deny.toml`, but adds `ttyr` as a direct dependency:
  the owner's call.

## Upstream

- **hjkl-form text fields wrap at `textwidth` under hjkl's default
  `formatoptions`.** Since hjkl 0.45 the default is `"tro"`, and
  `TextFieldEditor` builds its editor with `Options::default()`, so typing past
  column 79 with a space breaks the line; a single-line field's `text()` then
  returns only the first line. Verified with hjkl 0.46.0 by typing 145
  characters into a one-row field: 79 came back. `hodl_tui::keys::form_key` pins
  `FIELD_FORMATOPTIONS` to `"ro"` before every key (it must, because `set_text`
  rebuilds the editor). Fixing it in hjkl-form would let that go.
