// Copyright (c) 2018-2022 The Botho Foundation

//! Botho minting-related constants.

/// Nonce length.
pub const NONCE_LENGTH: usize = 64;

/// Maximum number of MintTx that may be included in a Block.
pub const MAX_MINT_TXS_PER_BLOCK: usize = 10;

/// Maximum number of MintConfigTx that may be included in a Block.
pub const MAX_MINT_CONFIG_TXS_PER_BLOCK: usize = 10;
