//! INACTIVE candidate primitives for #1293 / #1286. No production caller uses
//! these rules. Context parsing is NOT chain authentication; consumers must
//! authenticate provenance separately before using these helpers.
use crate::TxOutput;
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::Scalar;
use sha2::{Digest, Sha256, Sha512};

/// Candidate codec bound matching the existing default, NOT a current consensus
/// maximum. The V2 protocol must ratify this bound before any activation.
pub const CANDIDATE_MAX_AWARDS: usize = 4;
pub const KEM_BYTES: usize = 1088;
/// Version tag for the inactive payout derivation-context encoding.
///
/// This is deliberately not a live wire or storage version. A future V2
/// integration must authenticate the context through the chain commitment
/// before using it for ownership or signing.
pub const CONTEXT_VERSION: u8 = 2;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Encoding,
    Scalar,
    Point,
    Count,
    Exhausted,
    Ownership,
    Binding,
    /// A recovery context carried the zero tweak. With `T = 0` the "payout"
    /// target is its base target, so an ordinary output would pass `x·G = P`;
    /// direct outputs recover through [`recover_direct`] instead.
    ZeroTweak,
    /// A context tweak was not a canonical scalar (`T ≥ ℓ` little-endian).
    NonCanonicalTweak,
    /// A context encoding carried a version other than [`CONTEXT_VERSION`].
    Version,
}
pub type Hash = [u8; 32];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outpoint {
    pub hash: Hash,
    pub index: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub base_index: u32,
    pub tweak: Hash,
}

impl Context {
    /// Encode the context with an explicit version and fixed-width fields.
    /// Zero is valid here because an ordinary output starts a lineage with no
    /// accumulated payout tweak; payout records reject zero below.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        canonical_tweak(self.tweak)?;
        let mut out = Vec::with_capacity(1 + 4 + 32);
        out.push(CONTEXT_VERSION);
        out.extend(self.base_index.to_le_bytes());
        out.extend(self.tweak);
        Ok(out)
    }

    /// Decode exactly one canonical context. The bytes are syntax only; they
    /// do not authenticate provenance or chain membership.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader(bytes);
        let context = Self::decode_reader(&mut reader)?;
        if !reader.0.is_empty() {
            return Err(Error::Encoding);
        }
        Ok(context)
    }

    fn decode_reader(reader: &mut Reader<'_>) -> Result<Self, Error> {
        if reader.take::<1>()?[0] != CONTEXT_VERSION {
            return Err(Error::Version);
        }
        let context = Self {
            base_index: u32::from_le_bytes(reader.take()?),
            tweak: reader.take()?,
        };
        context.encode()?;
        Ok(context)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub outpoint: Outpoint,
    pub target: Hash,
    pub context: Context,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Award {
    pub winner: Outpoint,
    pub amount: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Domain {
    pub genesis: Hash,
    pub parent: Hash,
    pub height: u64,
    pub ordinary_root: Hash,
    pub manifest: Hash,
    pub ordinal: u32,
    pub amount: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub target: Hash,
    pub context: Context,
    pub counter: u8,
    pub delta: Hash,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub ordinal: u32,
    pub winner: Outpoint,
    pub amount: u64,
    pub target: Hash,
    pub public_key: Hash,
    pub ciphertext: Option<Vec<u8>>,
    pub context: Context,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub fees: u64,
    pub distributed: u64,
    pub burned: u64,
    pub seed: Hash,
}
fn scalar(bytes: Hash) -> Result<Scalar, Error> {
    Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes)).ok_or(Error::Scalar)
}
/// Canonical context tweak, with its own error so callers can tell a
/// malformed tweak from other scalar failures.
fn canonical_tweak(bytes: Hash) -> Result<Scalar, Error> {
    scalar(bytes).map_err(|_| Error::NonCanonicalTweak)
}
fn point(bytes: Hash) -> Result<RistrettoPublic, Error> {
    let p = RistrettoPublic::try_from(&bytes[..]).map_err(|_| Error::Point)?;
    if p.to_bytes() != bytes || bytes == [0; 32] {
        return Err(Error::Point);
    }
    Ok(p)
}
fn hash(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}
fn outpoint(v: &Outpoint, out: &mut Vec<u8>) {
    out.extend(v.hash);
    out.extend(v.index.to_le_bytes());
}
fn count(n: usize) -> Result<u32, Error> {
    if n > CANDIDATE_MAX_AWARDS {
        Err(Error::Count)
    } else {
        Ok(n as u32)
    }
}
/// Ordered manifest. No sorting: ordinal and source identity are consensus
/// inputs.
pub fn manifest(awards: &[Award]) -> Result<Hash, Error> {
    let mut out = b"BOTHO_LOTTERY_MANIFEST_V2\0".to_vec();
    out.extend(count(awards.len())?.to_le_bytes());
    for a in awards {
        outpoint(&a.winner, &mut out);
        out.extend(a.amount.to_le_bytes());
    }
    Ok(hash(&out))
}
/// Exact noncircular tweak preimage, excluding the final counter byte.
pub fn preimage(source: &Source, d: &Domain) -> Result<Vec<u8>, Error> {
    point(source.target)?;
    scalar(source.context.tweak)?;
    if d.ordinal >= CANDIDATE_MAX_AWARDS as u32 {
        return Err(Error::Count);
    }
    let mut out = b"BOTHO_LOTTERY_SPEND_TWEAK_V2\0".to_vec();
    out.extend(d.genesis);
    out.extend(d.parent);
    out.extend(d.height.to_le_bytes());
    out.extend(d.ordinary_root);
    out.extend(d.manifest);
    out.extend(d.ordinal.to_le_bytes());
    outpoint(&source.outpoint, &mut out);
    out.extend(source.target);
    out.extend(source.context.base_index.to_le_bytes());
    out.extend(source.context.tweak);
    out.extend(d.amount.to_le_bytes());
    Ok(out)
}
/// Derive the first valid candidate. No caller-controlled hash/counter
/// fallback.
pub fn derive(source: &Source, d: &Domain) -> Result<Derived, Error> {
    derive_inner(source, d, |bytes| Sha512::digest(bytes).into())
}
fn derive_inner(
    source: &Source,
    d: &Domain,
    digest: impl Fn(&[u8]) -> [u8; 64],
) -> Result<Derived, Error> {
    let p = point(source.target)?;
    let previous = scalar(source.context.tweak)?;
    let mut bytes = preimage(source, d)?;
    bytes.push(0);
    for counter in 0..=255u8 {
        *bytes.last_mut().unwrap() = counter;
        let delta = Scalar::from_bytes_mod_order_wide(&digest(&bytes));
        let cumulative = previous + delta;
        let delta_point = RistrettoPublic::from(&RistrettoPrivate::from(delta));
        let target = RistrettoPublic::from(p.as_ref() + delta_point.as_ref()).to_bytes();
        if delta != Scalar::ZERO && cumulative != Scalar::ZERO && target != [0; 32] {
            return Ok(Derived {
                target,
                context: Context {
                    base_index: source.context.base_index,
                    tweak: cumulative.to_bytes(),
                },
                counter,
                delta: delta.to_bytes(),
            });
        }
    }
    Err(Error::Exhausted)
}
/// A payout tweak: canonical ([`Error::NonCanonicalTweak`]) and nonzero
/// ([`Error::ZeroTweak`]).
fn payout_tweak(bytes: Hash) -> Result<Scalar, Error> {
    let tweak = canonical_tweak(bytes)?;
    if tweak == Scalar::ZERO {
        return Err(Error::ZeroTweak);
    }
    Ok(tweak)
}
/// `P − T·G`, rejected ([`Error::Point`]) when it is the identity.
fn untweak(target: &RistrettoPublic, tweak: Scalar) -> Result<Hash, Error> {
    let tweak_point = RistrettoPublic::from(&RistrettoPrivate::from(tweak));
    let base = RistrettoPublic::from(target.as_ref() - tweak_point.as_ref()).to_bytes();
    point(base)?;
    Ok(base)
}

// ---------------------------------------------------------------------------
// Shared wallet-side payout helpers (#1667 part 2).
//
// Every V2 payout consumer (the wasm-signer core, the bridge reserve scan,
// botho-wallet and the node wallet) applies the same three rules: validate the
// served context, scan the base target `P − T·G` at the context's base index,
// and recover `x_base + T` only if `x·G = P`. These helpers are the one copy
// of those rules. Each consumer keeps its own wire syntax (hex strictness,
// error strings) and its own ordinary ownership rule (the callbacks), which
// legitimately differ; the committed shared vector
// `tests/fixtures/lottery-v2-wallet.json` pins the observable behavior of
// every consumer against these helpers.
// ---------------------------------------------------------------------------

/// Validate the semantic fields of a served payout derivation context (the
/// `lotteryContext` RPC row, frozen #1641 contract) and build the [`Context`].
///
/// Rejects an unknown `version` ([`Error::Version`]), a non-canonical tweak
/// ([`Error::NonCanonicalTweak`]) and the zero tweak ([`Error::ZeroTweak`]): a
/// payout always carries a nonzero cumulative tweak, and a zero tweak would
/// make the "payout" an alias of its source (the #1450 legacy shape). This is
/// syntax only, NOT chain authentication: a forged context can only cause a
/// missed payout or a refused spend, never a wrong key (see
/// [`recover_with_context`]). The caller parses the tweak's wire encoding.
pub fn payout_context(version: u8, base_index: u32, tweak: Hash) -> Result<Context, Error> {
    if version != CONTEXT_VERSION {
        return Err(Error::Version);
    }
    payout_tweak(tweak)?;
    Ok(Context { base_index, tweak })
}

/// The base target `P − T·G` a payout with one-time key `target_key` (`P`) is
/// scanned against at `context.base_index`. This is exactly the base output
/// [`recover_with_context`] hands to its wallet callback.
///
/// Fails like [`recover_with_context`]: [`Error::NonCanonicalTweak`] /
/// [`Error::ZeroTweak`] for the tweak, [`Error::Point`] for a target that is
/// not a canonical non-identity point or an identity base.
pub fn base_target(target_key: &Hash, context: &Context) -> Result<Hash, Error> {
    let tweak = payout_tweak(context.tweak)?;
    untweak(&point(*target_key)?, tweak)
}

/// [`recover_with_context`] for a wallet whose base recovery also reports who
/// owns the base output (a subaddress index). Returns that owner with the
/// payout key; the owner is only returned when `x·G = P` holds.
///
/// This is the shared form of `detect_lottery_payout` (owner only) and
/// `recover_onetime_private_key` (key only).
pub fn recover_payout_owner<O>(
    output: &TxOutput,
    context: &Context,
    recover_base: impl FnOnce(&TxOutput, u32) -> Option<(O, RistrettoPrivate)>,
) -> Result<(O, RistrettoPrivate), Error> {
    let mut owner = None;
    let key = recover_with_context(output, context, |base, index| {
        let (found, key) = recover_base(base, index)?;
        owner = Some(found);
        Some(key)
    })?;
    Ok((owner.ok_or(Error::Ownership)?, key))
}

/// [`recover_direct`] with the base recovery's owner, like
/// [`recover_payout_owner`].
pub fn recover_direct_owner<O>(
    output: &TxOutput,
    base_index: u32,
    recover_base: impl FnOnce(&TxOutput, u32) -> Option<(O, RistrettoPrivate)>,
) -> Result<(O, RistrettoPrivate), Error> {
    let mut owner = None;
    let key = recover_direct(output, base_index, |base, index| {
        let (found, key) = recover_base(base, index)?;
        owner = Some(found);
        Some(key)
    })?;
    Ok((owner.ok_or(Error::Ownership)?, key))
}
/// Recover a V2 lottery payout through the existing wallet derivation
/// callback, then prove xG=P. This checks key ownership, not the chain
/// authentication of `context`.
///
/// Fails closed on a non-canonical tweak ([`Error::NonCanonicalTweak`]) and on
/// the zero tweak ([`Error::ZeroTweak`]): a payout always carries a nonzero
/// cumulative tweak ([`derive`] never yields zero and [`Record::encode`]
/// rejects it), while a zero tweak would let an ordinary output's base key pass
/// the ownership check. An unknown context version is rejected when the context
/// is decoded ([`Context::decode`], [`Record::decode`]: [`Error::Version`]).
/// Direct (untweaked) outputs recover through [`recover_direct`].
pub fn recover_with_context(
    output: &TxOutput,
    context: &Context,
    recover_base: impl FnOnce(&TxOutput, u32) -> Option<RistrettoPrivate>,
) -> Result<RistrettoPrivate, Error> {
    let tweak = payout_tweak(context.tweak)?;
    let target = point(output.target_key)?;
    point(output.public_key)?;
    let mut base = output.clone();
    base.target_key = untweak(&target, tweak)?;
    let key = recover_base(&base, context.base_index).ok_or(Error::Ownership)?;
    let base_secret: &Scalar = key.as_ref();
    let recovered = RistrettoPrivate::from(*base_secret + tweak);
    if RistrettoPublic::from(&recovered).to_bytes() != output.target_key {
        return Err(Error::Ownership);
    }
    Ok(recovered)
}
/// Recover a direct (untweaked) output at `base_index` through the same wallet
/// callback, with the same point checks and the same final xG=P proof as
/// [`recover_with_context`]. This is the lineage start that a zero-tweak
/// context used to express; it is a separate entry point so that a payout
/// context can never silently fall back to the base key.
pub fn recover_direct(
    output: &TxOutput,
    base_index: u32,
    recover_base: impl FnOnce(&TxOutput, u32) -> Option<RistrettoPrivate>,
) -> Result<RistrettoPrivate, Error> {
    point(output.target_key)?;
    point(output.public_key)?;
    let key = recover_base(output, base_index).ok_or(Error::Ownership)?;
    if RistrettoPublic::from(&key).to_bytes() != output.target_key {
        return Err(Error::Ownership);
    }
    Ok(key)
}
impl Record {
    /// Explicit canonical proposed wire bytes. Existing wire structs stay
    /// frozen.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.ordinal >= CANDIDATE_MAX_AWARDS as u32 {
            return Err(Error::Count);
        }
        point(self.target)?;
        point(self.public_key)?;
        if scalar(self.context.tweak)? == Scalar::ZERO {
            return Err(Error::Scalar);
        }
        let mut out = self.ordinal.to_le_bytes().to_vec();
        outpoint(&self.winner, &mut out);
        out.extend(self.amount.to_le_bytes());
        out.extend(self.target);
        out.extend(self.public_key);
        match &self.ciphertext {
            None => out.push(0),
            Some(ct) if ct.len() == KEM_BYTES => {
                out.push(1);
                out.extend(ct);
            }
            Some(_) => return Err(Error::Encoding),
        }
        out.extend(self.context.encode()?);
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader(bytes);
        let ordinal = u32::from_le_bytes(r.take()?);
        let winner = Outpoint {
            hash: r.take()?,
            index: u32::from_le_bytes(r.take()?),
        };
        let amount = u64::from_le_bytes(r.take()?);
        let target = r.take()?;
        let public_key = r.take()?;
        let ciphertext = match r.take::<1>()?[0] {
            0 => None,
            1 => Some(r.take::<KEM_BYTES>()?.to_vec()),
            _ => return Err(Error::Encoding),
        };
        let context = Context::decode_reader(&mut r)?;
        if !r.0.is_empty() {
            return Err(Error::Encoding);
        }
        let result = Self {
            ordinal,
            winner,
            amount,
            target,
            public_key,
            ciphertext,
            context,
        };
        if result.encode()? != bytes {
            return Err(Error::Encoding);
        }
        Ok(result)
    }
    /// Check the record against supplied source state and complete draw domain.
    /// The caller must authenticate source state and recompute the draw first.
    pub fn validate(
        &self,
        source: &Source,
        source_output: &TxOutput,
        d: &Domain,
    ) -> Result<(), Error> {
        self.encode()?;
        let expected = derive(source, d)?;
        if source_output.target_key != source.target
            || self.winner != source.outpoint
            || self.ordinal != d.ordinal
            || self.amount != d.amount
            || self.target != expected.target
            || self.context != expected.context
            || self.public_key != source_output.public_key
            || self.ciphertext != source_output.kem_ciphertext
        {
            return Err(Error::Binding);
        }
        Ok(())
    }
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let (head, tail) = self.0.split_at_checked(N).ok_or(Error::Encoding)?;
        self.0 = tail;
        head.try_into().map_err(|_| Error::Encoding)
    }
}
pub fn payout_root(records: &[Record]) -> Result<Hash, Error> {
    let mut out = b"BOTHO_LOTTERY_ROOT_V2\0".to_vec();
    out.extend(count(records.len())?.to_le_bytes());
    for (i, r) in records.iter().enumerate() {
        if r.ordinal as usize != i {
            return Err(Error::Binding);
        }
        out.extend(r.encode()?);
    }
    Ok(hash(&out))
}
pub fn summary_root(s: &Summary) -> Hash {
    let mut out = b"BOTHO_LOTTERY_SUMMARY_V2\0".to_vec();
    out.extend(s.fees.to_le_bytes());
    out.extend(s.distributed.to_le_bytes());
    out.extend(s.burned.to_le_bytes());
    out.extend(s.seed);
    hash(&out)
}
/// INACTIVE proposed body commitment. Does not change any live header
/// semantics.
pub fn body_root(minting: Hash, ordinary: Hash, payouts: Hash, summary: Hash) -> Hash {
    let mut out = b"BOTHO_BODY_ROOT_V2\0".to_vec();
    out.extend(minting);
    out.extend(ordinary);
    out.extend(payouts);
    out.extend(summary);
    hash(&out)
}
#[cfg(test)]
mod tests;
