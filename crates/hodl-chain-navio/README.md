# hodl-chain-navio

Navio (NAV) BLSCT support: EIP-2333 key derivation, the Navio key tree,
stealth sub-address derivation, and the modified-bech32 codec that
`nav1…` confidential addresses use.

Ported from [nav-io/navio-core](https://github.com/nav-io/navio-core) and
pinned by that project's own golden vectors — the ten mainnet addresses
`src/test/blsct/wallet/keyman_tests.cpp` asserts are reproduced byte for byte
in `tests/navio_core_vectors.rs`.

## What Navio is

Navio is the chain that succeeded NavCoin: a fresh Bitcoin-derivative that
launched in 2026 (genesis `0af3c23a…`) and is confidential by default. Its
supply is minted into **BLSCT** outputs — BLS signatures plus Confidential
Transactions over BLS12-381 — so a wallet's real receiving identity is a
stealth sub-address, not a script address. Segwit and taproot are also active
from genesis, so the transparent script layer exists too; that side is an
ordinary Electrum chain and lives in `hodl-chain-bitcoin`.

## Key tree

`SetHDSeed` in navio-core builds this from an EIP-2333 master key:

```
master ──child(130) ──┬── transaction(0) ──┬── view(0)
                      │                    └── spend(1)
                      ├── blinding(1)
                      └── token(2)
```

A receive address for `(account, index)` is then

```
m = Hs("SubAddress" || view || account || index)
D = spend_public + m*G      (the spend half)
C = view * D                (the view half)
address = bech32m_mod("nav", D_view || D_spend)
```

## Seed compatibility

navio-core derives the EIP-2333 master key two different ways depending on
whether the mnemonic carries a passphrase:

| passphrase | navio-core input               | hodl                              |
| ---------- | ------------------------------ | --------------------------------- |
| none       | the raw 32-byte BIP-39 entropy | [`BlsctKeys::from_bip39_entropy`] |
| set        | the 64-byte BIP-39 seed        | [`BlsctKeys::from_bip39_seed`]    |

hodl's vault stores only the stretched 64-byte seed, so `from_bip39_entropy`
is currently unreachable from the TUI. Since the empty-passphrase case is the
default, a hodl-derived BLSCT address would not match what navio-core or
navio-electrum show for the same mnemonic — and hodl cannot scan or spend
BLSCT outputs itself. Funds sent to such an address would be recoverable by no
shipping wallet, so **hodl does not surface a BLSCT receive address**. The
derivation is implemented and verified so that it is ready when either the
vault carries entropy or BLSCT scanning lands.

## Derivation path divergence (transparent side)

hodl derives Navio's transparent addresses at `m/84'/130'/0'` — 130 being
SLIP-44's registered NAV coin type, the index navio-core's BLSCT tree branches
at, and what NavCoin used in hodl before.

`nav-io/navio-electrum` sets `BIP44_COIN_TYPE = 0` and ships only
`m/44'|49'|84'/0'/0'`. Restoring a hodl mnemonic there shows an empty wallet
until a custom `m/84'/130'/0'` derivation is added. This affects the
transparent side only; BLSCT derivation is not BIP-32 and is unaffected.

## Not implemented

Spending BLSCT outputs, and scanning for them. Either would additionally need:

- Bulletproofs+ range proof serialization, transcript reconstruction and
  `MsgAmtCipher` decryption, for recovering an output's amount;
- the generator derivation (`range_proof::Generators`) those proofs are
  built against;
- set-membership proofs and BLS aggregate signing, for building a spend.

The Navio ElectrumX fork exposes the scan side through
`blockchain.block.get_txs_keys`, `blockchain.block.get_range_txs_keys` and
`blockchain.transaction.get_output`: per output it returns the blinding key,
the spending key and a 16-bit view tag, so a wallet tests ownership with one
G1 scalar multiplication per output (`nonce = blindingKey * viewKey`) before
fetching anything.

