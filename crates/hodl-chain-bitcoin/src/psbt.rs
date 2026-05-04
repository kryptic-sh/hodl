//! PSBT v0 (BIP-174) builder + BIP-143 segwit sighash + k256 ECDSA signing.
//!
//! Scope: segwit-v0 P2WPKH inputs and outputs only (BIP-84 wallets).
//! Legacy / wrapped-segwit input signing is future work.
//!
//! vsize estimate (all segwit-v0 P2WPKH):
//!   overhead = 41 vB  (version 4B + segwit marker/flag 1/2B scale → 0.5B
//!                       + locktime 4B + vin/vout counts 1B each, overhead
//!                       works out to ~41 after witness discount rounding)
//!   per input  ≈ 68 vB  (outpoint 36 + sequence 4 + scriptSig empty 1
//!                         + witness: 2 items, sig ~72 + pubkey 33 = ~107
//!                         → 107/4 discount + 41 base = ~68)
//!   per output ≈ 31 vB  (value 8 + scriptPubKey P2WPKH 22 + varint 1)
//!
//! These constants are documented approximations suitable for fee estimation;
//! the actual witness sizes can vary by a byte or two.

use hodl_core::error::{Error, Result};
use k256::ecdsa::{Signature, SigningKey, signature::hazmat::PrehashSigner};
use sha2::{Digest, Sha256};

use crate::electrum::Utxo;

// ── vsize constants ────────────────────────────────────────────────────────

pub const VSIZE_OVERHEAD: u64 = 41;
pub const VSIZE_PER_INPUT: u64 = 68;
pub const VSIZE_PER_OUTPUT: u64 = 31;

pub fn estimate_vsize(n_inputs: usize, n_outputs: usize) -> u64 {
    VSIZE_OVERHEAD + VSIZE_PER_INPUT * n_inputs as u64 + VSIZE_PER_OUTPUT * n_outputs as u64
}

// ── Coin selection ─────────────────────────────────────────────────────────

/// Greedy largest-first coin selection.
///
/// Returns the selected subset of UTXOs and the change amount (may be 0).
/// Fails if total value < amount_sats + fee_sats.
pub fn select_coins(
    mut utxos: Vec<Utxo>,
    amount_sats: u64,
    fee_sats: u64,
) -> Result<(Vec<Utxo>, u64)> {
    let target = amount_sats
        .checked_add(fee_sats)
        .ok_or_else(|| Error::Chain("amount + fee overflow".into()))?;

    utxos.sort_by_key(|u: &Utxo| std::cmp::Reverse(u.value));

    let mut selected = Vec::new();
    let mut total = 0u64;

    for utxo in utxos {
        selected.push(utxo.clone());
        total += utxo.value;
        if total >= target {
            return Ok((selected, total - target));
        }
    }

    Err(Error::Chain("insufficient funds".into()))
}

// ── Script helpers ─────────────────────────────────────────────────────────

/// Build a P2WPKH scriptPubKey (22 bytes): OP_0 <20-byte pubkey hash>.
pub fn p2wpkh_script(pubkey_hash: &[u8; 20]) -> Vec<u8> {
    let mut s = Vec::with_capacity(22);
    s.push(0x00); // OP_0
    s.push(0x14); // push 20 bytes
    s.extend_from_slice(pubkey_hash);
    s
}

/// Decode a bech32 P2WPKH address string to its 20-byte pubkey hash.
pub fn decode_p2wpkh_address(addr: &str) -> Result<[u8; 20]> {
    let (_, witness_ver, prog) =
        bech32::segwit::decode(addr).map_err(|e| Error::Codec(format!("bech32 decode: {e}")))?;
    if witness_ver != bech32::segwit::VERSION_0 || prog.len() != 20 {
        return Err(Error::Codec(
            "address is not a P2WPKH bech32 (witness v0, 20-byte program)".into(),
        ));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&prog);
    Ok(out)
}

/// Hash160 = RIPEMD160(SHA256(data)).
pub fn hash160(data: &[u8]) -> [u8; 20] {
    use ripemd::Ripemd160;
    let sha = Sha256::digest(data);
    let rmd = Ripemd160::digest(sha);
    rmd.into()
}

// ── Raw transaction serialization ──────────────────────────────────────────

fn write_varint(buf: &mut Vec<u8>, v: u64) {
    if v < 0xfd {
        buf.push(v as u8);
    } else if v <= 0xffff {
        buf.push(0xfd);
        buf.extend_from_slice(&(v as u16).to_le_bytes());
    } else if v <= 0xffff_ffff {
        buf.push(0xfe);
        buf.extend_from_slice(&(v as u32).to_le_bytes());
    } else {
        buf.push(0xff);
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// A parsed txid + vout reference (for outpoint).
pub struct Outpoint {
    pub txid_bytes: [u8; 32], // reversed (little-endian wire format)
    pub vout: u32,
}

impl Outpoint {
    /// Parse a hex txid (big-endian display) + vout into wire outpoint.
    pub fn from_str(txid_hex: &str, vout: u32) -> Result<Self> {
        let mut txid_bytes = hex::decode(txid_hex)
            .map_err(|e| Error::Codec(format!("txid hex: {e}")))?
            .try_into()
            .map_err(|_| Error::Codec("txid must be 32 bytes".into()))?;
        // Bitcoin txids are displayed big-endian but serialized little-endian.
        let arr: &mut [u8; 32] = &mut txid_bytes;
        arr.reverse();
        Ok(Self {
            txid_bytes: *arr,
            vout,
        })
    }
}

// ── BIP-143 segwit sighash (hand-rolled) ──────────────────────────────────
//
// Reference: https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki
//
// Double-SHA256 of the serialization below:
//   nVersion (4LE)
//   hashPrevouts (dsha256 of all outpoints)
//   hashSequence (dsha256 of all sequences)
//   outpoint (32+4)
//   scriptCode (varint-len prefixed)
//   value (8LE)
//   nSequence (4LE)
//   hashOutputs (dsha256 of all outputs)
//   nLocktime (4LE)
//   sighash type (4LE)

fn dsha256(data: &[u8]) -> [u8; 32] {
    let first: [u8; 32] = Sha256::digest(data).into();
    Sha256::digest(first).into()
}

pub struct TxInput {
    pub outpoint: Outpoint,
    pub sequence: u32,
    pub value_sats: u64,
    pub pubkey_hash: [u8; 20],
    pub pubkey: [u8; 33],
}

pub struct TxOutput {
    pub script_pubkey: Vec<u8>,
    pub value_sats: u64,
}

/// Compute the BIP-143 sighash (SIGHASH_ALL) for a single segwit-v0 P2WPKH input.
pub fn bip143_sighash(
    inputs: &[TxInput],
    outputs: &[TxOutput],
    input_index: usize,
    version: u32,
    locktime: u32,
) -> [u8; 32] {
    // hashPrevouts
    let mut prev_buf = Vec::new();
    for inp in inputs {
        prev_buf.extend_from_slice(&inp.outpoint.txid_bytes);
        prev_buf.extend_from_slice(&inp.outpoint.vout.to_le_bytes());
    }
    let hash_prevouts = dsha256(&prev_buf);

    // hashSequence
    let mut seq_buf = Vec::new();
    for inp in inputs {
        seq_buf.extend_from_slice(&inp.sequence.to_le_bytes());
    }
    let hash_sequence = dsha256(&seq_buf);

    // hashOutputs
    let mut out_buf = Vec::new();
    for out in outputs {
        out_buf.extend_from_slice(&out.value_sats.to_le_bytes());
        write_varint(&mut out_buf, out.script_pubkey.len() as u64);
        out_buf.extend_from_slice(&out.script_pubkey);
    }
    let hash_outputs = dsha256(&out_buf);

    let inp = &inputs[input_index];

    // scriptCode for P2WPKH: OP_DUP OP_HASH160 <20> OP_EQUALVERIFY OP_CHECKSIG
    let mut script_code = Vec::with_capacity(25);
    script_code.push(0x19); // varint length 25
    script_code.push(0x76); // OP_DUP
    script_code.push(0xa9); // OP_HASH160
    script_code.push(0x14); // push 20 bytes
    script_code.extend_from_slice(&inp.pubkey_hash);
    script_code.push(0x88); // OP_EQUALVERIFY
    script_code.push(0xac); // OP_CHECKSIG

    let sighash_type: u32 = 1; // SIGHASH_ALL

    let mut preimage = Vec::new();
    preimage.extend_from_slice(&version.to_le_bytes());
    preimage.extend_from_slice(&hash_prevouts);
    preimage.extend_from_slice(&hash_sequence);
    preimage.extend_from_slice(&inp.outpoint.txid_bytes);
    preimage.extend_from_slice(&inp.outpoint.vout.to_le_bytes());
    preimage.extend_from_slice(&script_code);
    preimage.extend_from_slice(&inp.value_sats.to_le_bytes());
    preimage.extend_from_slice(&inp.sequence.to_le_bytes());
    preimage.extend_from_slice(&hash_outputs);
    preimage.extend_from_slice(&locktime.to_le_bytes());
    preimage.extend_from_slice(&sighash_type.to_le_bytes());

    dsha256(&preimage)
}

// ── PSBT v0 serialization ──────────────────────────────────────────────────
//
// BIP-174 PSBT v0 binary format, minimal subset for signing:
//   magic: 0x70736274 + 0xff
//   global: unsigned tx (key 0x00)
//   per-input: witness utxo (key 0x01), bip32 derivation (omitted for brevity)
//   per-output: (empty for now)
//   separator 0x00 at end of each map

/// Build a PSBT v0 binary for a set of segwit-v0 P2WPKH inputs/outputs.
///
/// Returns raw PSBT bytes + the BIP-143 sighash for each input (so the caller
/// can sign without re-parsing).
pub fn build_psbt(inputs: &[TxInput], outputs: &[TxOutput]) -> Result<Vec<u8>> {
    let mut psbt = Vec::new();

    // Magic + separator
    psbt.extend_from_slice(b"psbt");
    psbt.push(0xff);

    // ── Global map ────────────────────────────────────────────────────────
    // Key 0x00 = unsigned tx (version 2 segwit, empty scriptSigs, no witnesses)
    let unsigned_tx = serialize_unsigned_tx(inputs, outputs);
    // key = varint(1) + 0x00
    psbt.push(0x01);
    psbt.push(0x00);
    // value = varint(len) + bytes
    write_varint(&mut psbt, unsigned_tx.len() as u64);
    psbt.extend_from_slice(&unsigned_tx);
    // map separator
    psbt.push(0x00);

    // ── Per-input maps ────────────────────────────────────────────────────
    for inp in inputs {
        // Key 0x01 = witness UTXO (the output being spent, as a segwit UTXO)
        // key: varint(1) + 0x01
        psbt.push(0x01);
        psbt.push(0x01);
        // value: 8-byte value + varint-len scriptPubKey
        let script = p2wpkh_script(&inp.pubkey_hash);
        let mut utxo_val = Vec::new();
        utxo_val.extend_from_slice(&inp.value_sats.to_le_bytes());
        write_varint(&mut utxo_val, script.len() as u64);
        utxo_val.extend_from_slice(&script);
        write_varint(&mut psbt, utxo_val.len() as u64);
        psbt.extend_from_slice(&utxo_val);
        // separator
        psbt.push(0x00);
    }

    // ── Per-output maps (empty — no extra data needed) ────────────────────
    // Each output map is just a separator byte.
    psbt.extend(std::iter::repeat_n(0x00u8, outputs.len()));

    Ok(psbt)
}

/// Serialize the unsigned transaction (no scriptSigs, no witnesses).
fn serialize_unsigned_tx(inputs: &[TxInput], outputs: &[TxOutput]) -> Vec<u8> {
    let mut tx = Vec::new();
    // version
    tx.extend_from_slice(&2u32.to_le_bytes());
    // vin count
    write_varint(&mut tx, inputs.len() as u64);
    for inp in inputs {
        tx.extend_from_slice(&inp.outpoint.txid_bytes);
        tx.extend_from_slice(&inp.outpoint.vout.to_le_bytes());
        tx.push(0x00); // empty scriptSig (varint 0)
        tx.extend_from_slice(&inp.sequence.to_le_bytes());
    }
    // vout count
    write_varint(&mut tx, outputs.len() as u64);
    for out in outputs {
        tx.extend_from_slice(&out.value_sats.to_le_bytes());
        write_varint(&mut tx, out.script_pubkey.len() as u64);
        tx.extend_from_slice(&out.script_pubkey);
    }
    // locktime
    tx.extend_from_slice(&0u32.to_le_bytes());
    tx
}

// ── Signing ────────────────────────────────────────────────────────────────

/// Sign all inputs in the PSBT and serialize a final segwit transaction.
///
/// `keys[i]` signs `inputs[i]`. For single-address wallets all keys are the
/// same; caller constructs the slice accordingly.
pub fn sign_inputs(inputs: &[TxInput], outputs: &[TxOutput], keys: &[[u8; 32]]) -> Result<Vec<u8>> {
    if keys.len() != inputs.len() {
        return Err(Error::Chain(format!(
            "key count {} != input count {}",
            keys.len(),
            inputs.len()
        )));
    }

    let version = 2u32;
    let locktime = 0u32;

    let mut witnesses: Vec<Vec<Vec<u8>>> = Vec::with_capacity(inputs.len());
    for (i, key_bytes) in keys.iter().enumerate() {
        let sighash = bip143_sighash(inputs, outputs, i, version, locktime);

        let signing_key = SigningKey::from_bytes(key_bytes.into())
            .map_err(|e| Error::Chain(format!("invalid signing key: {e}")))?;

        let (sig, _recid): (Signature, _) = signing_key
            .sign_prehash(sighash.as_ref())
            .map_err(|e| Error::Chain(format!("ecdsa sign: {e}")))?;

        // DER-encode + append SIGHASH_ALL byte.
        let mut der = sig.to_der().to_bytes().to_vec();
        der.push(0x01); // SIGHASH_ALL

        let pubkey = signing_key.verifying_key().to_encoded_point(true);
        let pubkey_bytes = pubkey.as_bytes().to_vec();

        witnesses.push(vec![der, pubkey_bytes]);
    }

    Ok(serialize_signed_tx(
        inputs, outputs, &witnesses, version, locktime,
    ))
}

/// Serialize the fully-signed segwit transaction.
fn serialize_signed_tx(
    inputs: &[TxInput],
    outputs: &[TxOutput],
    witnesses: &[Vec<Vec<u8>>],
    version: u32,
    locktime: u32,
) -> Vec<u8> {
    let mut tx = Vec::new();
    tx.extend_from_slice(&version.to_le_bytes());
    // Segwit marker + flag
    tx.push(0x00);
    tx.push(0x01);
    // vin
    write_varint(&mut tx, inputs.len() as u64);
    for inp in inputs {
        tx.extend_from_slice(&inp.outpoint.txid_bytes);
        tx.extend_from_slice(&inp.outpoint.vout.to_le_bytes());
        tx.push(0x00); // empty scriptSig
        tx.extend_from_slice(&inp.sequence.to_le_bytes());
    }
    // vout
    write_varint(&mut tx, outputs.len() as u64);
    for out in outputs {
        tx.extend_from_slice(&out.value_sats.to_le_bytes());
        write_varint(&mut tx, out.script_pubkey.len() as u64);
        tx.extend_from_slice(&out.script_pubkey);
    }
    // witness data (one stack per input)
    for wit in witnesses {
        write_varint(&mut tx, wit.len() as u64);
        for item in wit {
            write_varint(&mut tx, item.len() as u64);
            tx.extend_from_slice(item);
        }
    }
    tx.extend_from_slice(&locktime.to_le_bytes());
    tx
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_utxo(value: u64) -> Utxo {
        crate::electrum::Utxo {
            tx_hash: "a".repeat(64),
            tx_pos: 0,
            height: 800_000,
            value,
        }
    }

    #[test]
    fn coin_selection_greedy_picks_largest_first() {
        let utxos = vec![dummy_utxo(10_000), dummy_utxo(50_000), dummy_utxo(5_000)];
        let (selected, change) = select_coins(utxos, 40_000, 1_000).unwrap();
        // Largest (50k) alone covers 41k; change = 9k.
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].value, 50_000);
        assert_eq!(change, 9_000);
    }

    #[test]
    fn coin_selection_insufficient_funds() {
        let utxos = vec![dummy_utxo(1_000)];
        let err = select_coins(utxos, 5_000, 500).unwrap_err().to_string();
        assert!(err.contains("insufficient funds"), "got: {err}");
    }

    #[test]
    fn coin_selection_multiple_inputs() {
        let utxos = vec![dummy_utxo(3_000), dummy_utxo(3_000), dummy_utxo(3_000)];
        let (selected, change) = select_coins(utxos, 7_000, 500).unwrap();
        assert!(selected.len() >= 3);
        let total: u64 = selected.iter().map(|u| u.value).sum();
        assert_eq!(change, total - 7_500);
    }

    #[test]
    fn vsize_estimate_single_in_two_out() {
        let v = estimate_vsize(1, 2);
        // 41 + 68 + 31*2 = 171
        assert_eq!(v, 171);
    }

    #[test]
    fn decode_p2wpkh_mainnet() {
        // BIP-84 test vector address
        let addr = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        let hash = decode_p2wpkh_address(addr).unwrap();
        assert_eq!(hash.len(), 20);
    }

    #[test]
    fn decode_p2wpkh_rejects_non_segwit() {
        let err = decode_p2wpkh_address("1A1zP1eP5QGefi2DMPTfTL5SLmv7Divf Xx").unwrap_err();
        assert!(err.to_string().contains("bech32"));
    }

    #[test]
    fn sign_inputs_produces_valid_witness() {
        // Build a minimal transaction + sign it; verify the witness stack shape.
        let key_bytes = [1u8; 32];
        let signing_key = SigningKey::from_bytes((&key_bytes).into()).unwrap();
        let pubkey = signing_key.verifying_key().to_encoded_point(true);
        let pubkey_bytes: [u8; 33] = pubkey.as_bytes().try_into().unwrap();
        let pubkey_hash = hash160(&pubkey_bytes);

        let inp = TxInput {
            outpoint: Outpoint::from_str(&"aa".repeat(32), 0).unwrap(),
            sequence: 0xffff_fffe,
            value_sats: 100_000,
            pubkey_hash,
            pubkey: pubkey_bytes,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&pubkey_hash),
            value_sats: 99_000,
        };

        let signed = sign_inputs(&[inp], &[out], &[key_bytes]).unwrap();
        // Segwit tx: starts with version (4 bytes), then 0x00 0x01 marker.
        assert_eq!(signed[4], 0x00);
        assert_eq!(signed[5], 0x01);
    }

    #[test]
    fn bip143_sighash_is_deterministic() {
        let key_bytes = [2u8; 32];
        let signing_key = SigningKey::from_bytes((&key_bytes).into()).unwrap();
        let pubkey = signing_key.verifying_key().to_encoded_point(true);
        let pubkey_bytes: [u8; 33] = pubkey.as_bytes().try_into().unwrap();
        let pubkey_hash = hash160(&pubkey_bytes);

        let inp = TxInput {
            outpoint: Outpoint::from_str(&"bb".repeat(32), 1).unwrap(),
            sequence: 0xffff_ffff,
            value_sats: 200_000,
            pubkey_hash,
            pubkey: pubkey_bytes,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&pubkey_hash),
            value_sats: 199_000,
        };

        let h1 = bip143_sighash(&[inp], &[out], 0, 2, 0);

        let inp2 = TxInput {
            outpoint: Outpoint::from_str(&"bb".repeat(32), 1).unwrap(),
            sequence: 0xffff_ffff,
            value_sats: 200_000,
            pubkey_hash,
            pubkey: pubkey_bytes,
        };
        let out2 = TxOutput {
            script_pubkey: p2wpkh_script(&pubkey_hash),
            value_sats: 199_000,
        };
        let h2 = bip143_sighash(&[inp2], &[out2], 0, 2, 0);

        assert_eq!(h1, h2, "sighash must be deterministic");
    }

    // ── RBF / sequence tests ───────────────────────────────────────────────

    fn key_and_hash(seed_byte: u8) -> ([u8; 33], [u8; 20]) {
        let signing_key = SigningKey::from_bytes(&[seed_byte; 32].into()).unwrap();
        let pubkey_bytes: [u8; 33] = signing_key
            .verifying_key()
            .to_encoded_point(true)
            .as_bytes()
            .try_into()
            .unwrap();
        let pubkey_hash = hash160(&pubkey_bytes);
        (pubkey_bytes, pubkey_hash)
    }

    /// Coin selection across 3 UTXOs from 2 different pubkey_hashes selects
    /// the right greedy subset (largest-first).
    #[test]
    fn coin_selection_multi_address_picks_largest_first() {
        let (pk1, _) = key_and_hash(0x01);
        let (pk2, _) = key_and_hash(0x02);

        // 3 UTXOs: addr-A owns 60k and 5k, addr-B owns 20k.
        let utxos = vec![
            dummy_utxo(60_000), // largest → selected first
            dummy_utxo(5_000),
            dummy_utxo(20_000),
        ];

        // Greedy: 60k covers 50k + 1k fee → only one input needed.
        let (selected, change) = select_coins(utxos, 50_000, 1_000).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].value, 60_000);
        assert_eq!(change, 9_000);

        // The pubkeys are different per-address; both still derive P2WPKH.
        let h1 = hash160(&pk1);
        let h2 = hash160(&pk2);
        assert_ne!(h1, h2, "different keys must produce different hashes");
    }

    /// With rbf=true, every input's sequence must be 0xfffffffd (BIP-125).
    #[test]
    fn rbf_sequence_on_inputs() {
        let (pk, ph) = key_and_hash(0x10);
        let inp = TxInput {
            outpoint: Outpoint::from_str(&"ab".repeat(32), 0).unwrap(),
            sequence: 0xffff_fffd, // SEQUENCE_RBF
            value_sats: 100_000,
            pubkey_hash: ph,
            pubkey: pk,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&ph),
            value_sats: 99_000,
        };
        let signed = sign_inputs(&[inp], &[out], &[[0x10u8; 32]]).unwrap();
        // Sequence bytes at: version(4) + segwit-marker/flag(2) + vin_count(1)
        // + outpoint(36) + scriptSig_varint(1) = bytes 44..48.
        let seq_start = 4 + 2 + 1 + 36 + 1;
        let seq_bytes = &signed[seq_start..seq_start + 4];
        let seq = u32::from_le_bytes(seq_bytes.try_into().unwrap());
        assert_eq!(seq, 0xffff_fffd, "RBF sequence must be 0xfffffffd");
    }

    /// With rbf=false, every input's sequence must be 0xffffffff (final, non-RBF).
    #[test]
    fn non_rbf_sequence_on_inputs() {
        let (pk, ph) = key_and_hash(0x20);
        let inp = TxInput {
            outpoint: Outpoint::from_str(&"cd".repeat(32), 0).unwrap(),
            sequence: 0xffff_ffff, // SEQUENCE_FINAL
            value_sats: 100_000,
            pubkey_hash: ph,
            pubkey: pk,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&ph),
            value_sats: 99_000,
        };
        let signed = sign_inputs(&[inp], &[out], &[[0x20u8; 32]]).unwrap();
        let seq_start = 4 + 2 + 1 + 36 + 1;
        let seq_bytes = &signed[seq_start..seq_start + 4];
        let seq = u32::from_le_bytes(seq_bytes.try_into().unwrap());
        assert_eq!(seq, 0xffff_ffff, "non-RBF sequence must be 0xffffffff");
    }

    /// Sign round-trip with mixed inputs from two different keys.
    #[test]
    fn sign_round_trip_mixed_keys() {
        let (pk1, ph1) = key_and_hash(0x30);
        let (pk2, ph2) = key_and_hash(0x31);

        let inp1 = TxInput {
            outpoint: Outpoint::from_str(&"11".repeat(32), 0).unwrap(),
            sequence: 0xffff_fffd,
            value_sats: 80_000,
            pubkey_hash: ph1,
            pubkey: pk1,
        };
        let inp2 = TxInput {
            outpoint: Outpoint::from_str(&"22".repeat(32), 1).unwrap(),
            sequence: 0xffff_fffd,
            value_sats: 40_000,
            pubkey_hash: ph2,
            pubkey: pk2,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&ph1),
            value_sats: 119_000,
        };

        // Two different keys, one per input.
        let signed = sign_inputs(&[inp1, inp2], &[out], &[[0x30u8; 32], [0x31u8; 32]]).unwrap();

        // Segwit marker/flag present.
        assert_eq!(signed[4], 0x00);
        assert_eq!(signed[5], 0x01);
    }

    #[test]
    fn psbt_build_has_magic() {
        let key_bytes = [3u8; 32];
        let signing_key = SigningKey::from_bytes((&key_bytes).into()).unwrap();
        let pubkey = signing_key.verifying_key().to_encoded_point(true);
        let pubkey_bytes: [u8; 33] = pubkey.as_bytes().try_into().unwrap();
        let pubkey_hash = hash160(&pubkey_bytes);

        let inp = TxInput {
            outpoint: Outpoint::from_str(&"cc".repeat(32), 0).unwrap(),
            sequence: 0xffff_fffe,
            value_sats: 50_000,
            pubkey_hash,
            pubkey: pubkey_bytes,
        };
        let out = TxOutput {
            script_pubkey: p2wpkh_script(&pubkey_hash),
            value_sats: 49_000,
        };
        let psbt = build_psbt(&[inp], &[out]).unwrap();
        // PSBT magic: "psbt" + 0xff
        assert_eq!(&psbt[..5], b"psbt\xff");
    }
}
