//! JSON formats for private keys.
//! Files formatted in this way are sufficient to derive an account key in
//! a self-contained way without any context, which is useful for many tools.

use bth_account_keys::{RootEntropy, RootIdentity};
use serde::{Deserialize, Serialize};

/// Historical JSON schema for a root identity
#[derive(Clone, PartialEq, Eq, Hash, Default, Debug, Serialize, Deserialize)]
pub struct RootIdentityJson {
    /// Root entropy used to derive a user's private keys.
    pub root_entropy: [u8; 32],
    /// User's fog url (deprecated, ignored)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fog_url: String,
    /// User's report id (deprecated, ignored)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fog_report_id: String,
    /// User's fog authority spki bytes (deprecated, ignored)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fog_authority_spki: Vec<u8>,
}

impl From<&RootIdentity> for RootIdentityJson {
    fn from(src: &RootIdentity) -> Self {
        Self {
            root_entropy: src.root_entropy.bytes,
            fog_url: String::new(),
            fog_report_id: String::new(),
            fog_authority_spki: Vec::new(),
        }
    }
}

impl From<RootIdentityJson> for RootIdentity {
    fn from(src: RootIdentityJson) -> Self {
        // Note: fog fields are ignored - fog support removed
        Self {
            root_entropy: RootEntropy::from(&src.root_entropy),
        }
    }
}
