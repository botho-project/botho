//! Dev-only evaluation harness for issue #1715: Cryspen libcrux ML-KEM-768 /
//! ML-DSA-65 as a candidate replacement for RustCrypto `ml-kem` 0.2.3 /
//! `ml-dsa` 0.1.1.
//!
//! This crate is deliberately **outside** the Botho workspace (it declares its
//! own `[workspace]`), so nothing here enters the node's dependency graph or
//! the root `Cargo.lock`. It exposes thin byte-oriented adapters over both
//! implementations so the tests in `tests/`, the timing loop in
//! `examples/bench.rs`, and the fuzz target in `fuzz/` can drive them
//! symmetrically. See `docs/decisions/0013-libcrux-pq-evaluation.md` for the
//! results and the decision.

pub const KEM_PK: usize = 1184;
pub const KEM_SK: usize = 2400;
pub const KEM_CT: usize = 1088;
pub const KEM_SS: usize = 32;
pub const DSA_PK: usize = 1952;
pub const DSA_SK: usize = 4032;
pub const DSA_SIG: usize = 3309;

/// Which libcrux code path to exercise.
///
/// `Auto` is the public multiplexing API (runtime CPU detection), which is
/// what a production integration would call. The explicit variants pin one
/// backend so each can be cross-tested on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Auto,
    Portable,
    #[cfg(target_arch = "aarch64")]
    Neon,
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

impl Backend {
    /// Every backend compiled into this build. Explicit SIMD backends are
    /// only listed when the CPU actually supports them (calling the AVX2
    /// instantiation on a CPU without AVX2 is undefined behaviour).
    pub fn all() -> Vec<Backend> {
        #[allow(unused_mut)]
        let mut v = vec![Backend::Auto, Backend::Portable];
        #[cfg(target_arch = "aarch64")]
        if libcrux_platform::simd128_support() {
            v.push(Backend::Neon);
        }
        #[cfg(target_arch = "x86_64")]
        if libcrux_platform::simd256_support() {
            v.push(Backend::Avx2);
        }
        v
    }

    /// What `Auto` resolves to on this machine.
    pub fn auto_resolves_to() -> &'static str {
        if libcrux_platform::simd256_support() {
            "avx2"
        } else if libcrux_platform::simd128_support() {
            "neon"
        } else {
            "portable"
        }
    }
}

macro_rules! dispatch {
    ($backend:expr, $root:path, $f:ident ( $($arg:expr),* )) => {{
        use $root as root;
        match $backend {
            Backend::Auto => root::$f($($arg),*),
            Backend::Portable => root::portable::$f($($arg),*),
            #[cfg(target_arch = "aarch64")]
            Backend::Neon => root::neon::$f($($arg),*),
            #[cfg(target_arch = "x86_64")]
            Backend::Avx2 => root::avx2::$f($($arg),*),
        }
    }};
}

/// libcrux ML-KEM-768.
pub mod lc_kem {
    use super::*;
    use libcrux_ml_kem::mlkem768::{MlKem768Ciphertext, MlKem768PrivateKey, MlKem768PublicKey};

    /// FIPS 203 `ML-KEM.KeyGen_internal(d, z)`; `dz = d || z`.
    pub fn keygen(b: Backend, dz: [u8; 64]) -> ([u8; KEM_PK], [u8; KEM_SK]) {
        let kp = dispatch!(b, libcrux_ml_kem::mlkem768, generate_key_pair(dz));
        (*kp.pk(), *kp.sk())
    }

    /// FIPS 203 `ML-KEM.Encaps_internal(ek, m)`.
    pub fn encaps(b: Backend, pk: &[u8; KEM_PK], m: [u8; 32]) -> ([u8; KEM_CT], [u8; KEM_SS]) {
        let pk = MlKem768PublicKey::from(pk);
        let (ct, ss) = dispatch!(b, libcrux_ml_kem::mlkem768, encapsulate(&pk, m));
        (*ct.as_slice(), ss)
    }

    /// FIPS 203 `ML-KEM.Decaps_internal(dk, c)` (implicit rejection, never
    /// fails).
    pub fn decaps(b: Backend, sk: &[u8; KEM_SK], ct: &[u8; KEM_CT]) -> [u8; KEM_SS] {
        let sk = MlKem768PrivateKey::from(sk);
        let ct = MlKem768Ciphertext::from(ct);
        dispatch!(b, libcrux_ml_kem::mlkem768, decapsulate(&sk, &ct))
    }

    /// FIPS 203 §7.2 encapsulation-key input check (modulus check).
    pub fn validate_pk(b: Backend, pk: &[u8; KEM_PK]) -> bool {
        let pk = MlKem768PublicKey::from(pk);
        dispatch!(b, libcrux_ml_kem::mlkem768, validate_public_key(&pk))
    }

    /// FIPS 203 §7.3 decapsulation-key input check (hash check).
    pub fn validate_sk(b: Backend, sk: &[u8; KEM_SK]) -> bool {
        let sk = MlKem768PrivateKey::from(sk);
        match b {
            Backend::Auto => {
                // The multiplexed API only exposes the (sk, ct) form; the
                // ciphertext half is a pure length check, so any ct works.
                let ct = MlKem768Ciphertext::from([0u8; KEM_CT]);
                libcrux_ml_kem::mlkem768::validate_private_key(&sk, &ct)
            }
            Backend::Portable => libcrux_ml_kem::mlkem768::portable::validate_private_key_only(&sk),
            #[cfg(target_arch = "aarch64")]
            Backend::Neon => libcrux_ml_kem::mlkem768::neon::validate_private_key_only(&sk),
            #[cfg(target_arch = "x86_64")]
            Backend::Avx2 => libcrux_ml_kem::mlkem768::avx2::validate_private_key_only(&sk),
        }
    }
}

/// RustCrypto `ml-kem` 0.2.3 (the incumbent), driven deterministically.
pub mod rc_kem {
    use super::*;
    use ml_kem::{
        kem::Decapsulate, EncapsulateDeterministic, Encoded, EncodedSizeUser, KemCore, MlKem768,
        B32,
    };

    type Dk = <MlKem768 as KemCore>::DecapsulationKey;
    type Ek = <MlKem768 as KemCore>::EncapsulationKey;

    pub fn keygen(dz: [u8; 64]) -> ([u8; KEM_PK], [u8; KEM_SK]) {
        let d = B32::try_from(&dz[..32]).unwrap();
        let z = B32::try_from(&dz[32..]).unwrap();
        let (dk, ek) = MlKem768::generate_deterministic(&d, &z);
        let mut pk = [0u8; KEM_PK];
        pk.copy_from_slice(ek.as_bytes().as_slice());
        let mut sk = [0u8; KEM_SK];
        sk.copy_from_slice(dk.as_bytes().as_slice());
        (pk, sk)
    }

    pub fn encaps(pk: &[u8; KEM_PK], m: [u8; 32]) -> ([u8; KEM_CT], [u8; KEM_SS]) {
        let ek = Ek::from_bytes(&Encoded::<Ek>::try_from(&pk[..]).unwrap());
        let (ct, ss) = ek
            .encapsulate_deterministic(&B32::try_from(&m[..]).unwrap())
            .unwrap();
        (
            ct.as_slice().try_into().unwrap(),
            ss.as_slice().try_into().unwrap(),
        )
    }

    pub fn decaps(sk: &[u8; KEM_SK], ct: &[u8; KEM_CT]) -> [u8; KEM_SS] {
        let dk = Dk::from_bytes(&Encoded::<Dk>::try_from(&sk[..]).unwrap());
        let ct = ml_kem::Ciphertext::<MlKem768>::try_from(&ct[..]).unwrap();
        dk.decapsulate(&ct).unwrap().as_slice().try_into().unwrap()
    }
}

/// libcrux ML-DSA-65.
pub mod lc_dsa {
    use super::*;
    use libcrux_ml_dsa::ml_dsa_65::{MLDSA65Signature, MLDSA65SigningKey, MLDSA65VerificationKey};

    pub fn keygen(b: Backend, xi: [u8; 32]) -> ([u8; DSA_PK], [u8; DSA_SK]) {
        let kp = dispatch!(b, libcrux_ml_dsa::ml_dsa_65, generate_key_pair(xi));
        (*kp.verification_key.as_ref(), *kp.signing_key.as_ref())
    }

    /// FIPS 204 Algorithm 2 (pure, external interface) with explicit `rnd`.
    /// `rnd = [0; 32]` is the deterministic variant.
    pub fn sign(
        b: Backend,
        sk: &[u8; DSA_SK],
        msg: &[u8],
        ctx: &[u8],
        rnd: [u8; 32],
    ) -> Option<[u8; DSA_SIG]> {
        let sk = MLDSA65SigningKey::new(*sk);
        dispatch!(b, libcrux_ml_dsa::ml_dsa_65, sign(&sk, msg, ctx, rnd))
            .ok()
            .map(|s| *s.as_ref())
    }

    /// FIPS 204 Algorithm 3 (pure, external interface).
    pub fn verify(
        b: Backend,
        pk: &[u8; DSA_PK],
        msg: &[u8],
        ctx: &[u8],
        sig: &[u8; DSA_SIG],
    ) -> bool {
        let pk = MLDSA65VerificationKey::new(*pk);
        let sig = MLDSA65Signature::new(*sig);
        dispatch!(b, libcrux_ml_dsa::ml_dsa_65, verify(&pk, msg, ctx, &sig)).is_ok()
    }

    /// FIPS 204 Algorithm 7 `ML-DSA.Sign_internal` (ACVP "internal" interface).
    pub fn sign_internal(
        b: Backend,
        sk: &[u8; DSA_SK],
        msg: &[u8],
        rnd: [u8; 32],
    ) -> Option<[u8; DSA_SIG]> {
        let sk = MLDSA65SigningKey::new(*sk);
        dispatch!(b, libcrux_ml_dsa::ml_dsa_65, sign_internal(&sk, msg, rnd))
            .ok()
            .map(|s| *s.as_ref())
    }

    /// FIPS 204 Algorithm 8 `ML-DSA.Verify_internal`.
    pub fn verify_internal(b: Backend, pk: &[u8; DSA_PK], msg: &[u8], sig: &[u8; DSA_SIG]) -> bool {
        let pk = MLDSA65VerificationKey::new(*pk);
        let sig = MLDSA65Signature::new(*sig);
        dispatch!(
            b,
            libcrux_ml_dsa::ml_dsa_65,
            verify_internal(&pk, msg, &sig)
        )
        .is_ok()
    }
}

/// RustCrypto `ml-dsa` 0.1.1 (the incumbent).
pub mod rc_dsa {
    use super::*;
    use ml_dsa::{ExpandedSigningKey, MlDsa65, Signature, SigningKey, VerifyingKey, B32};

    pub fn keygen(xi: [u8; 32]) -> ([u8; DSA_PK], [u8; DSA_SK]) {
        let sk = SigningKey::<MlDsa65>::from_seed(&hybrid_array::Array::from(xi));
        let pk = sk.expanded_key().verifying_key().encode();
        #[allow(deprecated)]
        let esk = sk.expanded_key().to_expanded();
        (
            pk.as_slice().try_into().unwrap(),
            esk.as_slice().try_into().unwrap(),
        )
    }

    #[allow(deprecated)]
    fn esk(sk: &[u8; DSA_SK]) -> ExpandedSigningKey<MlDsa65> {
        ExpandedSigningKey::<MlDsa65>::from_expanded(
            &hybrid_array::Array::try_from(&sk[..]).unwrap(),
        )
    }

    fn vk(pk: &[u8; DSA_PK]) -> VerifyingKey<MlDsa65> {
        VerifyingKey::<MlDsa65>::decode(&hybrid_array::Array::try_from(&pk[..]).unwrap())
    }

    fn sig(sig: &[u8; DSA_SIG]) -> Option<Signature<MlDsa65>> {
        Signature::<MlDsa65>::decode(&hybrid_array::Array::try_from(&sig[..]).unwrap())
    }

    /// Deterministic external-interface signing (what Botho's `sign` does
    /// when `ctx` is empty).
    pub fn sign_deterministic(sk: &[u8; DSA_SK], msg: &[u8], ctx: &[u8]) -> Option<[u8; DSA_SIG]> {
        esk(sk)
            .sign_deterministic(msg, ctx)
            .ok()
            .map(|s| s.encode().as_slice().try_into().unwrap())
    }

    /// Hedged external-interface signing with an explicit `rnd`, built from
    /// `sign_internal` over `M' = 0 || |ctx| || ctx || M` (FIPS 204 Alg. 2).
    pub fn sign(sk: &[u8; DSA_SK], msg: &[u8], ctx: &[u8], rnd: [u8; 32]) -> Option<[u8; DSA_SIG]> {
        if ctx.len() > 255 {
            return None;
        }
        let prefix = [0u8, ctx.len() as u8];
        let s = esk(sk).sign_internal(&[&prefix, ctx, msg], &B32::from(rnd));
        Some(s.encode().as_slice().try_into().unwrap())
    }

    pub fn sign_internal(sk: &[u8; DSA_SK], msg: &[u8], rnd: [u8; 32]) -> [u8; DSA_SIG] {
        esk(sk)
            .sign_internal(&[msg], &B32::from(rnd))
            .encode()
            .as_slice()
            .try_into()
            .unwrap()
    }

    pub fn verify(pk: &[u8; DSA_PK], msg: &[u8], ctx: &[u8], s: &[u8; DSA_SIG]) -> bool {
        match sig(s) {
            Some(s) => vk(pk).verify_with_context(msg, ctx, &s),
            None => false,
        }
    }

    pub fn verify_internal(pk: &[u8; DSA_PK], msg: &[u8], s: &[u8; DSA_SIG]) -> bool {
        match sig(s) {
            Some(s) => vk(pk).verify_internal(msg, &s),
            None => false,
        }
    }
}

/// Botho-specific derivations that a migration must reproduce exactly.
pub mod botho {
    use rand_chacha::ChaCha20Rng;
    use rand_core::{RngCore, SeedableRng};

    /// `bth_crypto_pq::MlKem768KeyPair::from_seed(seed)` feeds a
    /// `ChaCha20Rng::from_seed(seed)` into RustCrypto `MlKem768::generate`,
    /// which draws `d` then `z` (32 bytes each; ml-kem 0.2.3 `kem.rs`
    /// `DecapsulationKey::generate`). A libcrux backend must reproduce these
    /// 64 bytes to keep every existing wallet's ML-KEM key (and therefore its
    /// v2 address) byte-identical.
    pub fn kem_seed_to_dz(seed: &[u8; 32]) -> [u8; 64] {
        let mut rng = ChaCha20Rng::from_seed(*seed);
        let mut d = [0u8; 32];
        let mut z = [0u8; 32];
        rng.fill_bytes(&mut d);
        rng.fill_bytes(&mut z);
        let mut dz = [0u8; 64];
        dz[..32].copy_from_slice(&d);
        dz[32..].copy_from_slice(&z);
        dz
    }
}

/// Tiny deterministic PRNG for test-input generation (not cryptographic).
pub struct TestRng(u64);

impl TestRng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
    pub fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut a = [0u8; N];
        self.fill(&mut a);
        a
    }
}
