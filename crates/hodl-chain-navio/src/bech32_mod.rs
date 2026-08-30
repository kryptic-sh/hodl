//! Navio's modified bech32 codec.
//!
//! BLSCT addresses carry a 96-byte double public key — far past what BIP-173's
//! 6-symbol checksum protects. Navio therefore ships a variant with an
//! 8-symbol checksum over a different BCH generator polynomial, chosen to
//! detect up to 5 errors in a 165-character string. Everything else (charset,
//! HRP expansion, the bech32/bech32m final constants) is unchanged from
//! upstream.
//!
//! Port of `src/blsct/bech32_mod.cpp` in nav-io/navio-core.

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// Data-part length, in 5-bit groups, of an encoded double public key:
/// `ceil(96 * 8 / 5)`.
pub const DOUBLE_PUBKEY_DATA_ENC_SIZE: usize = 154;

/// Checksum length in 5-bit groups. Upstream bech32 uses 6.
const CHECKSUM_SIZE: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Encoding {
    Bech32,
    Bech32m,
}

impl Encoding {
    fn constant(self) -> u64 {
        match self {
            Encoding::Bech32 => 1,
            Encoding::Bech32m => 0x2bc8_30a3,
        }
    }

    fn from_constant(c: u64) -> Option<Self> {
        match c {
            1 => Some(Encoding::Bech32),
            0x2bc8_30a3 => Some(Encoding::Bech32m),
            _ => None,
        }
    }
}

fn charset_rev(c: u8) -> Option<u8> {
    let idx = CHARSET.iter().position(|&x| x == c.to_ascii_lowercase())?;
    Some(idx as u8)
}

/// Remainder of the input polynomial modulo the generator, packed as eight
/// 5-bit groups in a 40-bit integer.
fn poly_mod(values: &[u8]) -> u64 {
    let mut c: u64 = 1;
    for &v in values {
        let c0 = (c >> 35) as u8;
        c = ((c & 0x0007_ffff_ffff) << 5) ^ u64::from(v);
        if c0 & 1 != 0 {
            c ^= 0x00f0_732d_c147;
        }
        if c0 & 2 != 0 {
            c ^= 0x00a8_b6df_a68e;
        }
        if c0 & 4 != 0 {
            c ^= 0x0019_3fab_c83c;
        }
        if c0 & 8 != 0 {
            c ^= 0x0032_2fd3_b451;
        }
        if c0 & 16 != 0 {
            c ^= 0x0064_0f37_688b;
        }
    }
    c
}

fn expand_hrp(hrp: &str) -> Vec<u8> {
    let bytes = hrp.as_bytes();
    let mut out = vec![0u8; bytes.len() * 2 + 1];
    for (i, &c) in bytes.iter().enumerate() {
        out[i] = c >> 5;
        out[i + bytes.len() + 1] = c & 0x1f;
    }
    out
}

fn create_checksum(encoding: Encoding, hrp: &str, values: &[u8]) -> [u8; CHECKSUM_SIZE] {
    let mut enc = expand_hrp(hrp);
    enc.extend_from_slice(values);
    enc.extend_from_slice(&[0u8; CHECKSUM_SIZE]);
    let m = poly_mod(&enc) ^ encoding.constant();
    let mut out = [0u8; CHECKSUM_SIZE];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = ((m >> (5 * (CHECKSUM_SIZE - 1 - i))) & 31) as u8;
    }
    out
}

fn verify_checksum(hrp: &str, values: &[u8]) -> Option<Encoding> {
    let mut enc = expand_hrp(hrp);
    enc.extend_from_slice(values);
    Encoding::from_constant(poly_mod(&enc))
}

/// Encode `values` (5-bit groups) under `hrp`.
///
/// Returns `None` unless `values` is exactly a double public key's worth of
/// data — this codec has no other callers and a short payload would produce a
/// string no decoder here accepts.
pub fn encode(encoding: Encoding, hrp: &str, values: &[u8]) -> Option<String> {
    if values.len() != DOUBLE_PUBKEY_DATA_ENC_SIZE {
        return None;
    }
    if hrp.bytes().any(|c| c.is_ascii_uppercase()) || !hrp.is_ascii() || hrp.is_empty() {
        return None;
    }
    let checksum = create_checksum(encoding, hrp, values);
    let mut out = String::with_capacity(hrp.len() + 1 + values.len() + CHECKSUM_SIZE);
    out.push_str(hrp);
    out.push('1');
    for &v in values.iter().chain(checksum.iter()) {
        out.push(CHARSET[v as usize] as char);
    }
    Some(out)
}

pub struct Decoded {
    pub encoding: Encoding,
    pub hrp: String,
    /// Payload in 5-bit groups, checksum stripped.
    pub data: Vec<u8>,
}

/// Decode a modified-bech32 string.
///
/// Mixed case is rejected, as in BIP-173. There is deliberately no 90-character
/// cap: this variant's strings are longer than that by construction.
pub fn decode(s: &str) -> Option<Decoded> {
    let mut has_lower = false;
    let mut has_upper = false;
    for c in s.bytes() {
        if c.is_ascii_lowercase() {
            has_lower = true;
        } else if c.is_ascii_uppercase() {
            has_upper = true;
        } else if !(33..=126).contains(&c) {
            return None;
        }
    }
    if has_lower && has_upper {
        return None;
    }

    let pos = s.rfind('1')?;
    // An empty HRP, or a data part shorter than the checksum, is not a valid
    // encoding.
    if pos == 0 || pos + 1 + CHECKSUM_SIZE > s.len() {
        return None;
    }

    let mut values = Vec::with_capacity(s.len() - 1 - pos);
    for c in s.as_bytes()[pos + 1..].iter() {
        values.push(charset_rev(*c)?);
    }
    let hrp = s[..pos].to_ascii_lowercase();
    let encoding = verify_checksum(&hrp, &values)?;
    values.truncate(values.len() - CHECKSUM_SIZE);
    Some(Decoded {
        encoding,
        hrp,
        data: values,
    })
}

/// Regroup `data` from 8-bit to 5-bit groups, zero-padding the final group.
pub fn convert_bits_8_to_5(data: &[u8]) -> Vec<u8> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(data.len().div_ceil(5) * 8);
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 31) as u8);
        }
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 31) as u8);
    }
    out
}

/// Regroup `data` from 5-bit to 8-bit groups. Rejects a non-zero or
/// over-long final padding, matching upstream's `ConvertBits<5, 8, false>`.
pub fn convert_bits_5_to_8(data: &[u8]) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(data.len() * 5 / 8);
    for &b in data {
        if b >> 5 != 0 {
            return None;
        }
        acc = (acc << 5) | u32::from(b);
        bits += 5;
        while bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    if bits >= 5 || (acc << (8 - bits)) & 0xff != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_values() -> Vec<u8> {
        (0..DOUBLE_PUBKEY_DATA_ENC_SIZE)
            .map(|i| (i % 32) as u8)
            .collect()
    }

    #[test]
    fn encode_decode_round_trip() {
        let values = dummy_values();
        let s = encode(Encoding::Bech32m, "nav", &values).unwrap();
        let d = decode(&s).unwrap();
        assert_eq!(d.encoding, Encoding::Bech32m);
        assert_eq!(d.hrp, "nav");
        assert_eq!(d.data, values);
    }

    #[test]
    fn bech32_and_bech32m_are_distinguished() {
        let values = dummy_values();
        let a = encode(Encoding::Bech32, "nav", &values).unwrap();
        let b = encode(Encoding::Bech32m, "nav", &values).unwrap();
        assert_ne!(a, b);
        assert_eq!(decode(&a).unwrap().encoding, Encoding::Bech32);
        assert_eq!(decode(&b).unwrap().encoding, Encoding::Bech32m);
    }

    #[test]
    fn checksum_is_eight_symbols() {
        let values = dummy_values();
        let s = encode(Encoding::Bech32m, "nav", &values).unwrap();
        assert_eq!(s.len(), 3 + 1 + DOUBLE_PUBKEY_DATA_ENC_SIZE + 8);
    }

    #[test]
    fn single_character_corruption_is_caught() {
        let values = dummy_values();
        let s = encode(Encoding::Bech32m, "nav", &values).unwrap();
        for i in 4..s.len() {
            let mut bytes = s.clone().into_bytes();
            // Swap to a different charset symbol.
            let cur = charset_rev(bytes[i]).unwrap();
            bytes[i] = CHARSET[((cur + 1) % 32) as usize];
            let corrupted = String::from_utf8(bytes).unwrap();
            assert!(decode(&corrupted).is_none(), "corruption at {i} not caught");
        }
    }

    #[test]
    fn mixed_case_rejected() {
        let values = dummy_values();
        let s = encode(Encoding::Bech32m, "nav", &values).unwrap();
        let mut mixed = s.clone();
        mixed.replace_range(0..1, "N");
        assert!(decode(&mixed).is_none());
    }

    #[test]
    fn empty_data_part_rejected() {
        // Upstream's bech32_mod_tests carries this exact case.
        assert!(decode("nav1").is_none());
    }

    #[test]
    fn convert_bits_round_trip() {
        let bytes: Vec<u8> = (0..96u16).map(|i| (i * 7 % 251) as u8).collect();
        let five = convert_bits_8_to_5(&bytes);
        assert_eq!(five.len(), DOUBLE_PUBKEY_DATA_ENC_SIZE);
        assert_eq!(convert_bits_5_to_8(&five).unwrap(), bytes);
    }

    #[test]
    fn convert_bits_rejects_dirty_padding() {
        let mut five = convert_bits_8_to_5(&[0u8; 96]);
        *five.last_mut().unwrap() = 1;
        assert!(convert_bits_5_to_8(&five).is_none());
    }
}
