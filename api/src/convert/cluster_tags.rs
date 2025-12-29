// Copyright (c) 2024 Botho Foundation

//! Convert to/from external::ClusterTagVector and related types.

use crate::{external, ConversionError};
use bth_transaction_core::{ClusterId, ClusterTagEntry, ClusterTagVector};

/// Convert ClusterTagVector --> external::ClusterTagVector.
impl From<&ClusterTagVector> for external::ClusterTagVector {
    fn from(source: &ClusterTagVector) -> Self {
        Self {
            entries: source.entries.iter().map(Into::into).collect(),
        }
    }
}

/// Convert external::ClusterTagVector --> ClusterTagVector.
impl TryFrom<&external::ClusterTagVector> for ClusterTagVector {
    type Error = ConversionError;

    fn try_from(source: &external::ClusterTagVector) -> Result<Self, Self::Error> {
        let entries = source
            .entries
            .iter()
            .map(ClusterTagEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ClusterTagVector { entries })
    }
}

/// Convert ClusterTagEntry --> external::ClusterTagEntry.
impl From<&ClusterTagEntry> for external::ClusterTagEntry {
    fn from(source: &ClusterTagEntry) -> Self {
        Self {
            cluster_id: Some((&source.cluster_id).into()),
            weight: source.weight,
        }
    }
}

/// Convert external::ClusterTagEntry --> ClusterTagEntry.
impl TryFrom<&external::ClusterTagEntry> for ClusterTagEntry {
    type Error = ConversionError;

    fn try_from(source: &external::ClusterTagEntry) -> Result<Self, Self::Error> {
        let cluster_id = source
            .cluster_id
            .as_ref()
            .map(|c| ClusterId::from(c))
            .ok_or(ConversionError::ObjectMissing)?;
        Ok(ClusterTagEntry {
            cluster_id,
            weight: source.weight,
        })
    }
}

/// Convert ClusterId --> external::ClusterId.
impl From<&ClusterId> for external::ClusterId {
    fn from(source: &ClusterId) -> Self {
        Self { id: source.0 }
    }
}

/// Convert external::ClusterId --> ClusterId.
impl From<&external::ClusterId> for ClusterId {
    fn from(source: &external::ClusterId) -> ClusterId {
        ClusterId(source.id)
    }
}
