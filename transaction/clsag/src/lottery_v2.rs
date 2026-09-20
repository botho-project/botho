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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Encoding,
    Scalar,
    Point,
    Count,
    Exhausted,
    Ownership,
    Binding,
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
/// Recover through the existing wallet derivation callback, then prove xG=P.
/// This checks key ownership, not the chain authentication of `context`.
pub fn recover_with_context(
    output: &TxOutput,
    context: &Context,
    recover_base: impl FnOnce(&TxOutput, u32) -> Option<RistrettoPrivate>,
) -> Result<RistrettoPrivate, Error> {
    let target = point(output.target_key)?;
    point(output.public_key)?;
    let tweak = scalar(context.tweak)?;
    let tweak_point = RistrettoPublic::from(&RistrettoPrivate::from(tweak));
    let mut base = output.clone();
    base.target_key = RistrettoPublic::from(target.as_ref() - tweak_point.as_ref()).to_bytes();
    point(base.target_key)?;
    let key = recover_base(&base, context.base_index).ok_or(Error::Ownership)?;
    let base_secret: &Scalar = key.as_ref();
    let recovered = RistrettoPrivate::from(*base_secret + tweak);
    if RistrettoPublic::from(&recovered).to_bytes() != output.target_key {
        return Err(Error::Ownership);
    }
    Ok(recovered)
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
        out.push(2);
        out.extend(self.context.base_index.to_le_bytes());
        out.extend(self.context.tweak);
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
        if r.take::<1>()?[0] != 2 {
            return Err(Error::Encoding);
        }
        let context = Context {
            base_index: u32::from_le_bytes(r.take()?),
            tweak: r.take()?,
        };
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
