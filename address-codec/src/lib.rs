// Copyright (c) 2024 Botho Foundation

//! Single shared base58 codec for Botho address strings.
//!
//! # Why this crate exists (ADR 0008, decision D5)
//!
//! Historically the `botho://…` address string was encoded and decoded by
//! **four independent** hand-rolled base58 implementations (the node, the
//! browser wasm-signer, the mobile FFI bridge, and the CLI wallet / desktop
//! shell). At the old 64-byte size a byte-for-byte drift between them was
//! unlikely; at the address-format-v2 size of ~3.2 KB (which now also carries
//! the post-quantum ML-KEM-768 and ML-DSA-65 public keys) a single mismatch
//! would silently make an address un-spendable on one client. This crate is the
//! ONE place that turns a [`PublicAddress`] into a `botho://2/<base58>` string
//! and back, so every encoder routes through identical logic and cannot
//! diverge.
//!
//! # Address format v2 (`botho://2/`)
//!
//! The base58 body is the fixed-length concatenation
//!
//! ```text
//! view(32) ‖ spend(32) ‖ kem(1184) ‖ dsa(1952)   = 3200 bytes
//! ```
//!
//! - `view`  — Ristretto subaddress view public key `C`
//! - `spend` — Ristretto subaddress spend public key `D`
//! - `kem`   — raw ML-KEM-768 public key ([`ML_KEM_768_PUBLIC_KEY_LEN`])
//! - `dsa`   — raw ML-DSA-65 public key ([`ML_DSA_65_PUBLIC_KEY_LEN`])
//!
//! The version prefix moves from `botho://1/` to **`botho://2/`** so that old
//! 64-byte v1 addresses fail loudly on decode rather than silently truncating
//! (ADR 0008 D2). The retired quantum-private prefixes (`botho://1q/`,
//! `botho-pq://1/`) are also rejected with a clear error.

#![deny(missing_docs)]
#![deny(unsafe_code)]

use core::fmt;

use bth_account_keys::{PublicAddress, ML_DSA_65_PUBLIC_KEY_LEN, ML_KEM_768_PUBLIC_KEY_LEN};
use bth_crypto_keys::RistrettoPublic;

/// Address-string format version encoded by this codec.
pub const ADDRESS_VERSION: u8 = 2;

/// Mainnet v2 address prefix.
pub const MAINNET_PREFIX: &str = "botho://2/";
/// Testnet v2 address prefix.
pub const TESTNET_PREFIX: &str = "tbotho://2/";

/// Retired v1 (classical, 64-byte) mainnet prefix — rejected on decode.
pub const MAINNET_V1_PREFIX: &str = "botho://1/";
/// Retired v1 (classical, 64-byte) testnet prefix — rejected on decode.
pub const TESTNET_V1_PREFIX: &str = "tbotho://1/";

/// Retired quantum-private mainnet prefix (ADR 0006) — rejected on decode.
pub const MAINNET_QUANTUM_PREFIX: &str = "botho://1q/";
/// Retired quantum-private testnet prefix (ADR 0006) — rejected on decode.
pub const TESTNET_QUANTUM_PREFIX: &str = "tbotho://1q/";
/// Retired legacy quantum prefix (ADR 0006) — rejected on decode.
pub const LEGACY_QUANTUM_PREFIX: &str = "botho-pq://1/";

/// Length of a single Ristretto public key in bytes.
const RISTRETTO_LEN: usize = 32;

/// Total length of the decoded v2 address body (`view‖spend‖kem‖dsa`).
pub const ADDRESS_BODY_LEN: usize =
    RISTRETTO_LEN * 2 + ML_KEM_768_PUBLIC_KEY_LEN + ML_DSA_65_PUBLIC_KEY_LEN;

// Byte offsets of each field within the decoded body.
const VIEW_START: usize = 0;
const SPEND_START: usize = VIEW_START + RISTRETTO_LEN;
const KEM_START: usize = SPEND_START + RISTRETTO_LEN;
const DSA_START: usize = KEM_START + ML_KEM_768_PUBLIC_KEY_LEN;

/// The network an address belongs to.
///
/// Defined locally so the codec stays a leaf crate (no dependency on the
/// heavier transaction-types crate). Callers map their own network enum to/from
/// this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    /// Main network (`botho://2/…`).
    Mainnet,
    /// Test network (`tbotho://2/…`).
    Testnet,
}

impl Network {
    /// The v2 address-string prefix for this network.
    pub fn v2_prefix(self) -> &'static str {
        match self {
            Network::Mainnet => MAINNET_PREFIX,
            Network::Testnet => TESTNET_PREFIX,
        }
    }
}

/// Errors returned when encoding or decoding a v2 address string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressCodecError {
    /// A retired quantum-private address (`botho://1q/`, `botho-pq://1/`) was
    /// supplied (ADR 0006).
    RetiredQuantumAddress,
    /// A retired v1 (classical, 64-byte) address (`botho://1/`) was supplied.
    /// v1 addresses cannot receive on the v2 chain (ADR 0008).
    RetiredV1Address,
    /// The string did not begin with a recognized v2 prefix.
    UnknownPrefix,
    /// The base58 body could not be decoded.
    InvalidBase58(String),
    /// The decoded body was not exactly [`ADDRESS_BODY_LEN`] bytes.
    WrongBodyLength {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length that was decoded.
        actual: usize,
    },
    /// The view sub-key was not a valid Ristretto point.
    InvalidViewKey,
    /// The spend sub-key was not a valid Ristretto point.
    InvalidSpendKey,
    /// The address being encoded did not carry both post-quantum keys at their
    /// exact raw lengths.
    MissingPqKeys {
        /// Actual ML-KEM public-key length on the address (expected
        /// [`ML_KEM_768_PUBLIC_KEY_LEN`]).
        kem_len: usize,
        /// Actual ML-DSA public-key length on the address (expected
        /// [`ML_DSA_65_PUBLIC_KEY_LEN`]).
        dsa_len: usize,
    },
}

impl fmt::Display for AddressCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressCodecError::RetiredQuantumAddress => write!(
                f,
                "quantum addresses retired (ADR 0006): the quantum-private \
                 transaction class was removed before mainnet, so this address \
                 can no longer receive funds. Ask the recipient for a current \
                 botho://2/ address."
            ),
            AddressCodecError::RetiredV1Address => write!(
                f,
                "address format v1 (botho://1/) retired (ADR 0008): v1 addresses \
                 carry no post-quantum keys and cannot receive on the v2 chain. \
                 Ask the recipient to regenerate a botho://2/ address."
            ),
            AddressCodecError::UnknownPrefix => write!(
                f,
                "unrecognized address prefix (expected botho://2/ or tbotho://2/)"
            ),
            AddressCodecError::InvalidBase58(e) => {
                write!(f, "invalid base58 in address body: {e}")
            }
            AddressCodecError::WrongBodyLength { expected, actual } => write!(
                f,
                "invalid address length: expected {expected} bytes, got {actual}"
            ),
            AddressCodecError::InvalidViewKey => write!(f, "invalid view public key"),
            AddressCodecError::InvalidSpendKey => write!(f, "invalid spend public key"),
            AddressCodecError::MissingPqKeys { kem_len, dsa_len } => write!(
                f,
                "cannot encode a v2 address without post-quantum keys: expected \
                 ML-KEM {ML_KEM_768_PUBLIC_KEY_LEN} / ML-DSA \
                 {ML_DSA_65_PUBLIC_KEY_LEN} bytes, got ML-KEM {kem_len} / ML-DSA \
                 {dsa_len}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AddressCodecError {}

/// Encode a [`PublicAddress`] as a `botho://2/<base58>` (or `tbotho://2/<…>`)
/// address string.
///
/// The address MUST carry both post-quantum public keys at their exact raw
/// lengths ([`ML_KEM_768_PUBLIC_KEY_LEN`] / [`ML_DSA_65_PUBLIC_KEY_LEN`]);
/// otherwise [`AddressCodecError::MissingPqKeys`] is returned. A classical-only
/// address cannot be represented in the v2 format.
pub fn encode_address(addr: &PublicAddress, network: Network) -> Result<String, AddressCodecError> {
    let kem = addr.kem_public_key();
    let dsa = addr.dsa_public_key();
    if kem.len() != ML_KEM_768_PUBLIC_KEY_LEN || dsa.len() != ML_DSA_65_PUBLIC_KEY_LEN {
        return Err(AddressCodecError::MissingPqKeys {
            kem_len: kem.len(),
            dsa_len: dsa.len(),
        });
    }

    let mut body = Vec::with_capacity(ADDRESS_BODY_LEN);
    body.extend_from_slice(&addr.view_public_key().to_bytes());
    body.extend_from_slice(&addr.spend_public_key().to_bytes());
    body.extend_from_slice(kem);
    body.extend_from_slice(dsa);
    debug_assert_eq!(body.len(), ADDRESS_BODY_LEN);

    let encoded = bs58::encode(&body).into_string();
    Ok(format!("{}{}", network.v2_prefix(), encoded))
}

/// Decode a `botho://2/<base58>` / `tbotho://2/<base58>` address string into a
/// [`PublicAddress`] and its [`Network`].
///
/// Validates, in order: retired-prefix rejection (quantum + v1), a recognized
/// v2 prefix, base58 decodability, the exact [`ADDRESS_BODY_LEN`], and that the
/// view/spend sub-keys are valid Ristretto points. The ML-KEM / ML-DSA byte
/// lengths are guaranteed by the total-length check and the fixed split
/// offsets.
pub fn decode_address(s: &str) -> Result<(PublicAddress, Network), AddressCodecError> {
    let s = s.trim();

    // Reject retired quantum-private addresses with a clear error (ADR 0006).
    if s.starts_with(MAINNET_QUANTUM_PREFIX)
        || s.starts_with(TESTNET_QUANTUM_PREFIX)
        || s.starts_with(LEGACY_QUANTUM_PREFIX)
    {
        return Err(AddressCodecError::RetiredQuantumAddress);
    }

    // Reject retired v1 (64-byte) addresses loudly (ADR 0008 D2). Checked before
    // the v2 prefix so `botho://1/…` never silently mis-parses.
    if s.starts_with(MAINNET_V1_PREFIX) || s.starts_with(TESTNET_V1_PREFIX) {
        return Err(AddressCodecError::RetiredV1Address);
    }

    // Match a v2 prefix (testnet first: `tbotho://2/` is not a prefix of
    // `botho://2/`, so order is not strictly required, but keep it explicit).
    let (encoded, network) = if let Some(rest) = s.strip_prefix(TESTNET_PREFIX) {
        (rest, Network::Testnet)
    } else if let Some(rest) = s.strip_prefix(MAINNET_PREFIX) {
        (rest, Network::Mainnet)
    } else {
        return Err(AddressCodecError::UnknownPrefix);
    };

    let body = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| AddressCodecError::InvalidBase58(e.to_string()))?;

    if body.len() != ADDRESS_BODY_LEN {
        return Err(AddressCodecError::WrongBodyLength {
            expected: ADDRESS_BODY_LEN,
            actual: body.len(),
        });
    }

    let view_key = RistrettoPublic::try_from(&body[VIEW_START..SPEND_START])
        .map_err(|_| AddressCodecError::InvalidViewKey)?;
    let spend_key = RistrettoPublic::try_from(&body[SPEND_START..KEM_START])
        .map_err(|_| AddressCodecError::InvalidSpendKey)?;

    let kem = body[KEM_START..DSA_START].to_vec();
    let dsa = body[DSA_START..].to_vec();
    debug_assert_eq!(kem.len(), ML_KEM_768_PUBLIC_KEY_LEN);
    debug_assert_eq!(dsa.len(), ML_DSA_65_PUBLIC_KEY_LEN);

    let addr = PublicAddress::new_with_pq(&spend_key, &view_key, kem, dsa);
    Ok((addr, network))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_util_from_random::FromRandom;
    use rand::rngs::StdRng;
    use rand_core::{RngCore, SeedableRng};

    /// Build a v2 address with deterministic (seeded) PQ payloads of the
    /// correct raw lengths. The bytes need not be valid PQ keys for the
    /// string encode/decode invariants (validity of the KEM/DSA keys is a
    /// producer concern, not the codec's).
    fn sample_v2_address(seed: u8) -> PublicAddress {
        let mut rng: StdRng = SeedableRng::from_seed([seed; 32]);
        let spend = RistrettoPublic::from_random(&mut rng);
        let view = RistrettoPublic::from_random(&mut rng);
        let mut kem = vec![0u8; ML_KEM_768_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut kem);
        let mut dsa = vec![0u8; ML_DSA_65_PUBLIC_KEY_LEN];
        rng.fill_bytes(&mut dsa);
        PublicAddress::new_with_pq(&spend, &view, kem, dsa)
    }

    /// `PublicAddress` does not implement `Debug` under the codec's
    /// `default-features = false` account-keys dependency, so assert address
    /// equality field-by-field instead of via `assert_eq!`.
    fn assert_addresses_eq(a: &PublicAddress, b: &PublicAddress) {
        assert_eq!(
            a.view_public_key().to_bytes(),
            b.view_public_key().to_bytes()
        );
        assert_eq!(
            a.spend_public_key().to_bytes(),
            b.spend_public_key().to_bytes()
        );
        assert_eq!(a.kem_public_key(), b.kem_public_key());
        assert_eq!(a.dsa_public_key(), b.dsa_public_key());
    }

    /// Decode and expect an error, without requiring `Debug` on the `Ok`
    /// `(PublicAddress, Network)` payload.
    fn expect_decode_err(s: &str) -> AddressCodecError {
        match decode_address(s) {
            Ok(_) => panic!("expected decode error for {s:?}"),
            Err(e) => e,
        }
    }

    /// Decode and expect success.
    fn expect_decode_ok(s: &str) -> (PublicAddress, Network) {
        match decode_address(s) {
            Ok(v) => v,
            Err(e) => panic!("expected decode success for {s:?}, got {e}"),
        }
    }

    #[test]
    fn body_len_is_3200() {
        assert_eq!(ADDRESS_BODY_LEN, 32 + 32 + 1184 + 1952);
        assert_eq!(ADDRESS_BODY_LEN, 3200);
    }

    #[test]
    fn round_trips_both_networks() {
        for network in [Network::Mainnet, Network::Testnet] {
            let addr = sample_v2_address(1);
            let s = encode_address(&addr, network).expect("encode");
            assert!(s.starts_with(network.v2_prefix()));
            let (decoded, decoded_net) = expect_decode_ok(&s);
            assert_eq!(decoded_net, network);
            assert_addresses_eq(&decoded, &addr);
            // Re-encoding the decoded address reproduces the exact string.
            assert_eq!(encode_address(&decoded, network).unwrap(), s);
        }
    }

    #[test]
    fn mainnet_and_testnet_prefixes_are_distinct() {
        let addr = sample_v2_address(2);
        let main = encode_address(&addr, Network::Mainnet).unwrap();
        let test = encode_address(&addr, Network::Testnet).unwrap();
        assert!(main.starts_with("botho://2/"));
        assert!(test.starts_with("tbotho://2/"));
        // Only the prefix differs; the base58 body is identical.
        assert_eq!(
            main.strip_prefix("botho://2/").unwrap(),
            test.strip_prefix("tbotho://2/").unwrap()
        );
    }

    #[test]
    fn encode_rejects_classical_only_address() {
        let mut rng: StdRng = SeedableRng::from_seed([3u8; 32]);
        let addr = PublicAddress::new(
            &RistrettoPublic::from_random(&mut rng),
            &RistrettoPublic::from_random(&mut rng),
        );
        let err = encode_address(&addr, Network::Mainnet).expect_err("no pq keys");
        assert!(matches!(err, AddressCodecError::MissingPqKeys { .. }));
    }

    #[test]
    fn rejects_old_v1_address_loudly() {
        // A well-formed 64-byte v1 body under the old prefix must fail loudly,
        // not silently truncate.
        let body = vec![0u8; 64];
        let v1 = format!("botho://1/{}", bs58::encode(&body).into_string());
        assert_eq!(expect_decode_err(&v1), AddressCodecError::RetiredV1Address);
        let tv1 = format!("tbotho://1/{}", bs58::encode(&body).into_string());
        assert_eq!(expect_decode_err(&tv1), AddressCodecError::RetiredV1Address);
    }

    #[test]
    fn rejects_retired_quantum_addresses() {
        for a in [
            "botho://1q/3sampleBase58Payload",
            "tbotho://1q/3sampleBase58Payload",
            "botho-pq://1/3sampleBase58Payload",
        ] {
            assert_eq!(
                expect_decode_err(a),
                AddressCodecError::RetiredQuantumAddress
            );
        }
    }

    #[test]
    fn rejects_wrong_length_body() {
        // Correct v2 prefix but a too-short body.
        let short = format!("botho://2/{}", bs58::encode([1u8; 100]).into_string());
        assert!(matches!(
            expect_decode_err(&short),
            AddressCodecError::WrongBodyLength { .. }
        ));
    }

    #[test]
    fn rejects_wrong_version_prefix() {
        let addr = sample_v2_address(4);
        let s = encode_address(&addr, Network::Mainnet).unwrap();
        let body = s.strip_prefix("botho://2/").unwrap();
        // Same body under a bogus v3 prefix.
        let v3 = format!("botho://3/{body}");
        assert_eq!(expect_decode_err(&v3), AddressCodecError::UnknownPrefix);
    }

    #[test]
    fn rejects_corrupted_base58() {
        let addr = sample_v2_address(5);
        let mut s = encode_address(&addr, Network::Mainnet).unwrap();
        // '0' (zero) is not in the bitcoin base58 alphabet.
        s.push('0');
        s.push('I');
        assert!(matches!(
            expect_decode_err(&s),
            AddressCodecError::InvalidBase58(_)
        ));
    }

    // End-to-end with the seed-derived key hierarchy (sub-issue 2): a derived
    // account's address encodes to `botho://2/…`, decodes back to a
    // `PublicAddress`, and the recovered PQ keys still decapsulate / verify.
    #[test]
    fn derived_address_round_trips_and_pq_keys_still_work() {
        use bth_account_keys::AccountKey;
        use bth_crypto_pq::{derive_pq_keys_from_seed, MlDsa65PublicKey, MlKem768PublicKey};

        let seed = [17u8; 64];
        let mut rng: StdRng = SeedableRng::from_seed([9u8; 32]);
        let account_key = AccountKey::new(
            &bth_crypto_keys::RistrettoPrivate::from_random(&mut rng),
            &bth_crypto_keys::RistrettoPrivate::from_random(&mut rng),
        )
        .attach_pq_from_seed(&seed);

        let addr = account_key.default_subaddress();
        assert!(addr.has_pq_keys());

        // Encode → decode round-trip preserves the whole address.
        let s = encode_address(&addr, Network::Mainnet).expect("encode derived");
        let (decoded, net) = expect_decode_ok(&s);
        assert_eq!(net, Network::Mainnet);
        assert_addresses_eq(&decoded, &addr);

        // The decoded KEM key still encapsulates to a secret the derived
        // keypair decapsulates.
        let pq = derive_pq_keys_from_seed(&seed);
        let kem = MlKem768PublicKey::from_bytes(decoded.kem_public_key())
            .expect("decoded KEM key is well formed");
        let (ciphertext, sender_secret) = kem.encapsulate();
        let recipient_secret = pq
            .kem_keypair
            .decapsulate(&ciphertext)
            .expect("decapsulation with derived secret");
        assert_eq!(sender_secret.as_bytes(), recipient_secret.as_bytes());

        // The decoded DSA key still verifies a signature from the derived key.
        let msg = b"botho address v2 round trip";
        let sig = pq.sig_keypair.sign(msg);
        let dsa = MlDsa65PublicKey::from_bytes(decoded.dsa_public_key())
            .expect("decoded DSA key is well formed");
        assert!(dsa.verify(msg, &sig).is_ok());
    }
}
