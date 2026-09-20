//! Strict parsers for the audited v4 IDL pinned by the executed Tier-2 fixture.
use super::*;
use crate::solana_rpc::SolanaAccount;
use sha2::{Digest, Sha256};

pub fn discriminator(name: &str) -> [u8; 8] {
    Sha256::digest(format!("account:{name}").as_bytes())[..8]
        .try_into()
        .unwrap()
}
pub fn checked<'a>(
    account: &'a SolanaAccount,
    owner: Pubkey,
    name: &str,
) -> Result<&'a [u8], String> {
    if account.owner != owner
        || account.executable
        || !account.data.starts_with(&discriminator(name))
    {
        return Err(format!("invalid {name} owner/discriminator"));
    }
    Ok(&account.data[8..])
}
pub struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("truncated Squads account".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn key(&mut self) -> Result<Pubkey, String> {
        Ok(Pubkey(self.take(32)?.try_into().unwrap()))
    }
    pub fn count(&mut self) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > 256 {
            return Err("Squads vector exceeds bound".into());
        }
        Ok(n)
    }
    pub fn keys(&mut self) -> Result<Vec<Pubkey>, String> {
        let n = self.count()?;
        (0..n).map(|_| self.key()).collect()
    }
}
pub struct MultisigState {
    pub create_key: Pubkey,
    pub config_authority: Pubkey,
    pub threshold: u16,
    pub time_lock: u32,
    pub index: u64,
    pub stale_index: u64,
    pub members: Vec<(Pubkey, u8)>,
}
impl MultisigState {
    pub fn parse(a: &SolanaAccount) -> Result<Self, String> {
        let mut r = Reader::new(checked(a, SQUADS_V4_PROGRAM_ID, "Multisig")?);
        let create_key = r.key()?;
        let config_authority = r.key()?;
        let threshold = r.u16()?;
        let time_lock = r.u32()?;
        let index = r.u64()?;
        let stale_index = r.u64()?;
        match r.byte()? {
            0 => {}
            1 => {
                r.key()?;
            }
            _ => return Err("invalid rent collector option".into()),
        };
        r.byte()?;
        let count = r.count()?;
        let members = (0..count)
            .map(|_| Ok((r.key()?, r.byte()?)))
            .collect::<Result<Vec<_>, String>>()?;
        if members.windows(2).any(|w| w[0].0 >= w[1].0) {
            return Err("unsorted/duplicate Squads members".into());
        }
        Ok(Self {
            create_key,
            config_authority,
            threshold,
            time_lock,
            index,
            stale_index,
            members,
        })
    }
}
pub struct VaultState {
    pub multisig: Pubkey,
    pub creator: Pubkey,
    pub index: u64,
    pub vault_index: u8,
    pub message: SquadsTransactionMessage,
}
impl VaultState {
    pub fn parse(a: &SolanaAccount) -> Result<Self, String> {
        let mut r = Reader::new(checked(a, SQUADS_V4_PROGRAM_ID, "VaultTransaction")?);
        let multisig = r.key()?;
        let creator = r.key()?;
        let index = r.u64()?;
        r.byte()?;
        let vault_index = r.byte()?;
        r.byte()?;
        if r.u32()? != 0 {
            return Err("ephemeral signers not supported for bridge proposals".into());
        }
        let num_signers = r.byte()?;
        let num_writable_signers = r.byte()?;
        let num_writable_non_signers = r.byte()?;
        let account_keys = r.keys()?;
        let n = r.count()?;
        let mut instructions = vec![];
        for _ in 0..n {
            let program_id_index = r.byte()?;
            let n = r.count()?;
            let account_indexes = r.take(n)?.to_vec();
            let n = r.u32()? as usize;
            if n > 1232 {
                return Err("instruction too large".into());
            }
            let data = r.take(n)?.to_vec();
            instructions.push(SquadsCompiledInstruction {
                program_id_index,
                account_indexes,
                data,
            });
        }
        if r.u32()? != 0 {
            return Err("address tables not supported for bridge proposals".into());
        }
        Ok(Self {
            multisig,
            creator,
            index,
            vault_index,
            message: SquadsTransactionMessage {
                num_signers,
                num_writable_signers,
                num_writable_non_signers,
                account_keys,
                instructions,
            },
        })
    }
}
pub struct ProposalState {
    pub multisig: Pubkey,
    pub index: u64,
    pub status: u8,
    pub timestamp: i64,
    pub approved: Vec<Pubkey>,
}
impl ProposalState {
    pub fn parse(a: &SolanaAccount) -> Result<Self, String> {
        let mut r = Reader::new(checked(a, SQUADS_V4_PROGRAM_ID, "Proposal")?);
        let multisig = r.key()?;
        let index = r.u64()?;
        let status = r.byte()?;
        if status > 6 || status == 4 {
            return Err("invalid/transient proposal status".into());
        }
        let timestamp = r.u64()? as i64;
        r.byte()?;
        let approved = r.keys()?;
        let _rejected = r.keys()?;
        let _cancelled = r.keys()?;
        if approved.windows(2).any(|w| w[0] >= w[1]) {
            return Err("duplicate proposal approval".into());
        }
        Ok(Self {
            multisig,
            index,
            status,
            timestamp,
            approved,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn account(name: &str, body: Vec<u8>) -> SolanaAccount {
        let mut data = discriminator(name).to_vec();
        data.extend(body);
        SolanaAccount {
            owner: SQUADS_V4_PROGRAM_ID,
            executable: false,
            lamports: 1,
            data,
        }
    }
    fn multisig() -> SolanaAccount {
        let mut b = vec![0; 64];
        b.extend(2u16.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(7u64.to_le_bytes());
        b.extend(5u64.to_le_bytes());
        b.extend([0, 255]);
        b.extend(3u32.to_le_bytes());
        for n in 1..=3 {
            b.extend([n; 32]);
            b.push(7)
        }
        account("Multisig", b)
    }
    #[test]
    fn rejects_untrusted_metadata_and_every_truncated_multisig_prefix() {
        let a = multisig();
        let p = MultisigState::parse(&a).unwrap();
        assert_eq!(p.threshold, 2);
        assert_eq!(p.index, 7);
        assert_eq!(p.members.len(), 3);
        for length in 0..a.data.len() {
            let mut corrupt = a.clone();
            corrupt.data.truncate(length);
            assert!(MultisigState::parse(&corrupt).is_err(), "prefix {length}");
        }
        let mut corrupt = a.clone();
        corrupt.owner = Pubkey([5; 32]);
        assert!(MultisigState::parse(&corrupt).is_err());
        let mut corrupt = a.clone();
        corrupt.executable = true;
        assert!(MultisigState::parse(&corrupt).is_err());
        let mut corrupt = a.clone();
        corrupt.data[0] ^= 1;
        assert!(MultisigState::parse(&corrupt).is_err());
        let mut corrupt = a.clone();
        let start = corrupt.data.len() - 33;
        corrupt.data[start..start + 32].fill(1);
        assert!(MultisigState::parse(&corrupt).is_err());
    }
    #[test]
    fn rejects_unbounded_and_unsupported_transaction_features() {
        let mut body = vec![0; 75];
        body.extend(0u32.to_le_bytes());
        body.extend([0, 0, 0]);
        body.extend(0u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        let a = account("VaultTransaction", body);
        assert!(VaultState::parse(&a).is_ok());
        let mut bad = a.clone();
        bad.data[83..87].copy_from_slice(&1u32.to_le_bytes());
        assert!(VaultState::parse(&bad).is_err(), "ephemeral signer");
        let mut bad = a.clone();
        let n = bad.data.len();
        bad.data[n - 4..].copy_from_slice(&1u32.to_le_bytes());
        assert!(VaultState::parse(&bad).is_err(), "ALT");
        let mut bad = a.clone();
        bad.data[90..94].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(VaultState::parse(&bad).is_err(), "unbounded keys");
        for length in 0..a.data.len() {
            let mut bad = a.clone();
            bad.data.truncate(length);
            assert!(VaultState::parse(&bad).is_err(), "prefix {length}");
        }
    }
    #[test]
    fn proposal_status_and_duplicate_approval_validation() {
        let mut body = vec![0; 40];
        body.push(3);
        body.extend(1u64.to_le_bytes());
        body.push(255);
        body.extend(2u32.to_le_bytes());
        body.extend([1; 32]);
        body.extend([2; 32]);
        body.extend(0u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        let a = account("Proposal", body);
        assert_eq!(ProposalState::parse(&a).unwrap().status, 3);
        let mut bad = a.clone();
        bad.data[48] = 4;
        assert!(ProposalState::parse(&bad).is_err());
        let mut bad = a.clone();
        bad.data[94..126].fill(1);
        assert!(ProposalState::parse(&bad).is_err());
    }
}
