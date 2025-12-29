// Copyright (c) 2018-2022 The Botho Foundation

//! LMDB utilities / common features.

mod metadata_store;

pub use metadata_store::{
    MetadataStore, MetadataStoreError, MetadataStoreSettings, MetadataVersion,
};
