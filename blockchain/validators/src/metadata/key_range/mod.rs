// Copyright (c) 2018-2022 The Botho Foundation

//! See [validator].

pub mod config;
pub mod validator;

pub use self::{
    config::{Config, KeyValidity, KeyValidityMap},
    validator::KeyRangeValidator,
};
