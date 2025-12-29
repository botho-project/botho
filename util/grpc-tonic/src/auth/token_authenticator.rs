// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Tonic authenticator that relies on a shared secret for generating and
//! verifying tokens.

use super::*;

use displaydoc::Display;
use hmac::{Hmac, Mac};
use bth_common::time::TimeProvider;
use sha2::Sha256;
use std::time::Duration;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// Token-based authentication: An object that implements `Authenticator`,
/// allowing to authenticate users using HMAC-generated tokens.
pub struct TokenAuthenticator<TP: TimeProvider> {
    /// Secret shared between the authenticator and then token generator,
    /// allowing for generated tokens to be cryptographically-verified by
    /// the authenticator.
    shared_secret: [u8; 32],

    /// The maximum duration a token is valid for.
    max_token_lifetime: Duration,

    /// Time provider.
    time_provider: TP,
}

impl<TP: TimeProvider> Drop for TokenAuthenticator<TP> {
    fn drop(&mut self) {
        self.shared_secret.zeroize();
    }
}

// Manual Send+Sync implementation since we ensure the type is thread-safe
// The time_provider uses interior mutability but MockTimeProvider ensures this
unsafe impl<TP: TimeProvider + Send> Send for TokenAuthenticator<TP> {}
unsafe impl<TP: TimeProvider + Sync> Sync for TokenAuthenticator<TP> {}

impl<TP: TimeProvider + Send + Sync> Authenticator for TokenAuthenticator<TP> {
    fn authenticate(
        &self,
        maybe_credentials: Option<BasicCredentials>,
    ) -> Result<String, AuthenticatorError> {
        let credentials = maybe_credentials.ok_or(AuthenticatorError::Unauthenticated)?;
        let mut parts = credentials.password.split(':');
        let username = parts
            .next()
            .ok_or(AuthenticatorError::InvalidAuthorizationToken)?;
        let timestamp = parts
            .next()
            .ok_or(AuthenticatorError::InvalidAuthorizationToken)?;
        let signature = parts
            .next()
            .ok_or(AuthenticatorError::InvalidAuthorizationToken)?;
        if parts.next().is_some() {
            return Err(AuthenticatorError::InvalidAuthorizationToken);
        }
        if username != credentials.username {
            return Err(AuthenticatorError::InvalidAuthorizationToken);
        }
        if !self.is_valid_time(timestamp)? {
            return Err(AuthenticatorError::ExpiredAuthorizationToken);
        }
        if !self.is_valid_signature(&format!("{username}:{timestamp}"), signature)? {
            return Err(AuthenticatorError::InvalidAuthorizationToken);
        }
        Ok(credentials.username)
    }
}

impl<TP: TimeProvider> TokenAuthenticator<TP> {
    /// Create a new Token authenticator
    ///
    /// Arguments:
    /// * shared_secret: The shared secret which is used as a key to hmac to
    ///   create tokens
    /// * max_token_lifetime: The duration of time that the tokens that we hand
    ///   out are valid for
    /// * time_provider: A generic object that provides "Duration since the
    ///   epoch"
    pub fn new(shared_secret: [u8; 32], max_token_lifetime: Duration, time_provider: TP) -> Self {
        Self {
            shared_secret,
            max_token_lifetime,
            time_provider,
        }
    }

    fn is_valid_time(&self, timestamp: &str) -> Result<bool, AuthenticatorError> {
        let token_time: Duration = Duration::from_secs(
            timestamp
                .parse()
                .map_err(|_| AuthenticatorError::InvalidAuthorizationToken)?,
        );
        let our_time = self
            .time_provider
            .since_epoch()
            .map_err(|_| AuthenticatorError::ExpiredAuthorizationToken)?;
        let distance: Duration = our_time
            .checked_sub(token_time)
            .unwrap_or_else(|| token_time - our_time);
        Ok(distance < self.max_token_lifetime)
    }

    fn is_valid_signature(&self, data: &str, signature: &str) -> Result<bool, AuthenticatorError> {
        let their_suffix: Vec<u8> =
            hex::decode(signature).map_err(|_| AuthenticatorError::InvalidAuthorizationToken)?;

        let mut mac = Hmac::<Sha256>::new_from_slice(&self.shared_secret)
            .map_err(|_| AuthenticatorError::Other("Invalid HMAC key".to_owned()))?;
        mac.update(data.as_bytes());
        let our_signature = mac.finalize().into_bytes();

        let our_suffix: &[u8] = &our_signature[..10];
        Ok(bool::from(our_suffix.ct_eq(&their_suffix)))
    }
}

/// Error values for token generator.
#[derive(Display, Debug)]
pub enum TokenBasicCredentialsGeneratorError {
    /// TimeProvider error
    TimeProvider,

    /// Invalid HMAC key
    InvalidHmacKey,
}

/// Token generator - an object that can generate HMAC authentication tokens.
pub struct TokenBasicCredentialsGenerator<TP: TimeProvider> {
    shared_secret: [u8; 32],
    time_provider: TP,
}

impl<TP: TimeProvider> TokenBasicCredentialsGenerator<TP> {
    /// Create a new token credential generator
    ///
    /// Arguments:
    /// * shared_secret: The shared secret used as hmac key
    /// * time_provider: A generic object that provides "Duration since the
    ///   epoch"
    pub fn new(shared_secret: [u8; 32], time_provider: TP) -> Self {
        Self {
            shared_secret,
            time_provider,
        }
    }

    /// Generate a token for a user-id
    pub fn generate_for(
        &self,
        user_id: &str,
    ) -> Result<BasicCredentials, TokenBasicCredentialsGeneratorError> {
        let current_time_seconds = self
            .time_provider
            .since_epoch()
            .map_err(|_| TokenBasicCredentialsGeneratorError::TimeProvider)?
            .as_secs();
        let prefix = format!("{user_id}:{current_time_seconds}");

        let mut mac = Hmac::<Sha256>::new_from_slice(&self.shared_secret)
            .map_err(|_| TokenBasicCredentialsGeneratorError::InvalidHmacKey)?;
        mac.update(prefix.as_bytes());
        let signature = mac.finalize().into_bytes();

        Ok(BasicCredentials::new(
            user_id,
            &format!(
                "{}:{}:{}",
                user_id,
                current_time_seconds,
                hex::encode(&signature[..10])
            ),
        ))
    }
}

impl<TP: TimeProvider> Drop for TokenBasicCredentialsGenerator<TP> {
    fn drop(&mut self) {
        self.shared_secret.zeroize();
    }
}
