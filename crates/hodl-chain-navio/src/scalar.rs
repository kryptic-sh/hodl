//! Big-endian scalar conversions matching Navio's `MclScalar`.
//!
//! `MclScalar::SetVch` / `MclScalar(const uint256&)` feed bytes to
//! `mclBnFr_setBigEndianMod`, i.e. big-endian with a reduction mod `r`, and
//! `GetVch` serializes big-endian in 32 bytes. `bls12_381::Scalar` is
//! little-endian throughout, so every crossing goes through here.

use bls12_381::Scalar;

/// Interpret `bytes` as a big-endian integer and reduce it mod `r`.
///
/// Accepts up to 64 bytes — the two lengths Navio uses are 32 (a `uint256`
/// hash) and 48 (EIP-2333's OKM). Longer input would silently lose its high
/// limbs, so it panics instead.
pub fn scalar_from_be_bytes(bytes: &[u8]) -> Scalar {
    assert!(bytes.len() <= 64, "scalar input longer than 64 bytes");
    let mut wide = [0u8; 64];
    // `from_bytes_wide` reads little-endian, so reverse into the low end.
    for (i, b) in bytes.iter().rev().enumerate() {
        wide[i] = *b;
    }
    Scalar::from_bytes_wide(&wide)
}

/// Serialize `s` as 32 big-endian bytes.
pub fn scalar_to_be_bytes(s: &Scalar) -> [u8; 32] {
    let mut out = s.to_bytes();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_small_value() {
        let s = scalar_from_be_bytes(
            &[0u8; 31]
                .iter()
                .chain([1u8].iter())
                .copied()
                .collect::<Vec<_>>(),
        );
        assert_eq!(s, Scalar::from(1u64));
        let be = scalar_to_be_bytes(&s);
        assert_eq!(be[31], 1);
        assert!(be[..31].iter().all(|b| *b == 0));
    }

    #[test]
    fn reduces_modulo_r() {
        // r for BLS12-381, big-endian. Feeding it in must yield zero.
        let r = hex::decode("73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000001")
            .unwrap();
        assert_eq!(scalar_from_be_bytes(&r), Scalar::from(0u64));
    }

    #[test]
    fn accepts_48_byte_input() {
        let okm = [0xffu8; 48];
        // Must not panic and must land in the field.
        let s = scalar_from_be_bytes(&okm);
        assert_eq!(scalar_from_be_bytes(&scalar_to_be_bytes(&s)), s);
    }
}
