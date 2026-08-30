//! Golden vectors lifted from nav-io/navio-core's own test suite.
//!
//! `src/test/blsct/wallet/keyman_tests.cpp` seeds a BLSCT wallet with the
//! master scalar 1 (`SetHDSeed(MclScalar(uint256(uint64_t{1})))`) and asserts
//! the first ten mainnet receive addresses of account 0. Reproducing them end
//! to end pins the whole stack: EIP-2333 derivation, the key tree, the
//! sub-address hash preimage, G1 serialization, and the bech32-mod codec.

use hodl_chain_navio::keys::BlsctKeys;
use hodl_chain_navio::scalar::scalar_from_be_bytes;
use hodl_chain_navio::{MAINNET_HRP, decode_address, encode_address};

/// Expected addresses for account 0, sub-address indices 0..10.
const EXPECTED: [&str; 10] = [
    "nav14h85k6mf4l5fu3j4v0nuuswjwrz5entvzcw9jl3s8uknsndu0pfzaze4992n36uq7hpcy8yeuu854p0gmhq4m2u0tf5znazc527cxy4j7c39qxlc89wg4nca8pazkecx0p6wmu3pwrma3ercgrk8s7k4759q2thyq5",
    "nav1kq8zphgry92d02j7sm460c8xv88avuxcqlxrl7unxva9c4uawuvskx3s3pd6g3nychcq0ksy0tlpmgyt35384dnqdtudafa00yrjpcsffef404xur6cegkm98llf5xptkj6napgqk6g9dpa0x24qe4cgaqj2j0wl9p",
    "nav1s48u8dtxseguw6s7ecewph2szrxwy3fzx47rzdtppgzrnxrp0p0peetjx5h2f6gpwy3ar65tmw4p39z30pzt0t6san07th0694pffc0f6dghnskfujfanzwjdzep8fn0ezdeg7ejmvulj8nymrzkw8wdvqc3mqvnpw",
    "nav1k34crux0s5urxtcndupcd37ehkakkz6da8n5ghmx388vfynhqa4k9zmrp8qmyw485ujvpkjwcasqhq5rqpxrkvhm0tg3ap3er8eycgwu5ew5xq5u84vzxsaqgc37ud67g5j9jvynlqacx78zl6l2flw82g02a3z4g5",
    "nav13qq8el3522u4jxd4e8y54du9d5fqlqlcmz8n90k8hc6e72dqky99ajgfarmd3puzx9zz9hazr99zrggharvuh9ulg9ugnu6nf5hfvq9mw03nv2g9xz9v2vnvn6uumrwxcv93ae54kuzjmz49g4mx0u2pzqftvrhu8f",
    "nav1kh6n54xfhq0nmsr8rrqsff8xtegr8hvsdsvn2sdtk3w25w9fkescwqeqlnasm9ngcr895ycxx4ave2m5crya7hgyydhsa66ct995lrvywpgseu8cq4yjwcjm7dkh367pg3dhtxnwsfsct7my5tzu0c8jwsst6luayt",
    "nav15gxjtgw289m82any2fn75gdh09cyte4c6qlzrms7wr4a4vyqdd8epl2qncrhspdflru3kcc4kdpzrrqtcvrq3qzxdjrh3l2lqr9v5jnjw22ut4axj9czcajj8pfyy0mm99n0q8088z99uame7ckrk8k3yvp7dxdw8q",
    "nav1j08knwnjcuukjl88vyt06c2h7unqjurflvtqaa9ljw08mz6swp2je7zg962u5qke9dc3cnhz3rkfdg0uhyw3zw6jk2akd08krzxqms74lcm9paapjygl3kglru3gaumy682qysl2hy6cgujqs9ugfxvqzcza5h00tj",
    "nav15vn8346nl5ttuu28w7dhwetq5vlu8tv3dgdqdhks769ye9gd9ssaszk5unwtejp6vftw82936k20m93sc4z9z29zz4f2rneexfw770ducywzxt3wp6vc7c3lhgxn2jxxufv74hwppcxd3prcn2yf2qgk6sg4u3f74j",
    "nav1kag0sqeuzz64stxmc5ztrafqvyx7lv4k09leasauyku5eg6zdsh23nyauzwrszyqysj02ecqmzkdrdym02w7u5y6ed7ptwe5adqyqufnqfj5hqve2et935gw8p8jculfnr66qpk8u86f35zaxs053920gsyneqtgdc",
];

/// The upstream fixture's master key.
///
/// `MclScalar(uint256(uint64_t{1}))` is not the scalar 1: `uint256`'s
/// uint64 constructor lays the value out little-endian, and `MclScalar`'s
/// `uint256` constructor then reads those bytes *big*-endian
/// (`mclBnFr_setBigEndianMod`). The leading `0x01` therefore lands in the most
/// significant byte, making the master key 2^248.
fn keys() -> BlsctKeys {
    let mut master_be = [0u8; 32];
    master_be[0] = 1;
    BlsctKeys::from_master_scalar(&scalar_from_be_bytes(&master_be))
}

#[test]
fn first_ten_receive_addresses_match_navio_core() {
    let km = keys();
    for (index, expected) in EXPECTED.iter().enumerate() {
        let dpk = km.sub_address(0, index as u64);
        let encoded = encode_address(MAINNET_HRP, &dpk).expect("encodes");
        assert_eq!(&encoded, expected, "sub-address {index} diverged");
    }
}

#[test]
fn addresses_round_trip_through_the_decoder() {
    for expected in EXPECTED {
        let dpk = decode_address(MAINNET_HRP, expected).expect("decodes");
        assert_eq!(encode_address(MAINNET_HRP, &dpk).unwrap(), expected);
    }
}

#[test]
fn decoder_rejects_the_wrong_network() {
    assert!(decode_address("tnv", EXPECTED[0]).is_none());
}

#[test]
fn decoder_rejects_a_flipped_character() {
    let mut s = EXPECTED[0].to_string();
    let last = s.pop().unwrap();
    s.push(if last == 'q' { 'p' } else { 'q' });
    assert!(decode_address(MAINNET_HRP, &s).is_none());
}

/// Upstream's null-key regtest addresses encode the identity in one or both
/// halves. `blsct::DoublePublicKey::HasNonIdentityKeys` exists because outputs
/// built for those are anyone-can-spend, so the decoder must not hand one back
/// as a usable destination.
#[test]
fn decoder_rejects_identity_keys() {
    const NULL_KEY: &str = "rnv1cqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqwwvmtas";
    const NULL_VIEW_KEY: &str = "rnv1cqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqp9l36wnnr97hjsnf2cuvf756cr7rdzxyl9m5hyz6zn368ut3htzcd327s0le0gdwl7e67q9dkgkxhvmdqls40d";
    const NULL_SPEND_KEY: &str = "rnv1jlca8fe3jltegf54vwxyl2dvplpk3rz0ja6tjpdpfcar79cm43vxc40g8luh5xh0lva0qzkmytrthsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqma0f57ul";
    for addr in [NULL_KEY, NULL_VIEW_KEY, NULL_SPEND_KEY] {
        assert!(
            decode_address("rnv", addr).is_none(),
            "identity-keyed address accepted: {addr}"
        );
    }
}

/// Sub-addresses are per-account: the same index under a different account
/// must not collide.
#[test]
fn accounts_are_separated() {
    let km = keys();
    let receive = km.sub_address(0, 0);
    let change = km.sub_address(-1, 0);
    let staking = km.sub_address(-2, 0);
    assert_ne!(receive, change);
    assert_ne!(receive, staking);
    assert_ne!(change, staking);
}
