//! JSON formats for private keys.
//! Files formatted in this way are sufficient to derive an account key in
//! a self-contained way without any context, which is useful for many tools.

use bth_account_keys::{RootEntropy, RootIdentity};
use serde::{Deserialize, Serialize};

/// JSON schema for a root identity
#[derive(Clone, PartialEq, Eq, Hash, Default, Debug, Serialize, Deserialize)]
pub struct RootIdentityJson {
    /// Root entropy used to derive a user's private keys.
    pub root_entropy: [u8; 32],
}

impl From<&RootIdentity> for RootIdentityJson {
    fn from(src: &RootIdentity) -> Self {
        Self {
            root_entropy: src.root_entropy.bytes,
        }
    }
}

impl From<RootIdentityJson> for RootIdentity {
    fn from(src: RootIdentityJson) -> Self {
        Self {
            root_entropy: RootEntropy::from(&src.root_entropy),
        }
    }
}
