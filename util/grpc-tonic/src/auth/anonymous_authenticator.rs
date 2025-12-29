// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Tonic authenticator that authenticates everything as an anonymous user.

use super::*;

/// The username returned for all authenticate calls.
pub const ANONYMOUS_USER: &str = "<anonymous>";

/// A trivial authenticator object that authenticates everyone as "anonymous"
#[derive(Default)]
pub struct AnonymousAuthenticator;

impl Authenticator for AnonymousAuthenticator {
    fn authenticate(
        &self,
        _maybe_credentials: Option<BasicCredentials>,
    ) -> Result<String, AuthenticatorError> {
        Ok(ANONYMOUS_USER.to_owned())
    }
}
