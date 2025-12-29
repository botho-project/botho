// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Tonic-based gRPC authentication utilities.

mod anonymous_authenticator;
mod token_authenticator;

pub use anonymous_authenticator::{AnonymousAuthenticator, ANONYMOUS_USER};
pub use token_authenticator::{
    TokenAuthenticator, TokenBasicCredentialsGenerator, TokenBasicCredentialsGeneratorError,
};

use base64::{engine::general_purpose::STANDARD as BASE64_ENGINE, Engine};
use displaydoc::Display;
use std::str;
use tonic::{metadata::MetadataMap, Request, Status};

/// Error values for authentication.
#[derive(Display, Debug, PartialEq)]
pub enum AuthenticatorError {
    /// Unauthenticated
    Unauthenticated,

    /// Invalid user authorization token
    InvalidAuthorizationToken,

    /// Expired user authorization token
    ExpiredAuthorizationToken,

    /// Authorization header error: {0}
    AuthorizationHeader(AuthorizationHeaderError),

    /// Other: {0}
    Other(String),
}

impl From<AuthorizationHeaderError> for AuthenticatorError {
    fn from(src: AuthorizationHeaderError) -> Self {
        Self::AuthorizationHeader(src)
    }
}

impl From<AuthenticatorError> for Status {
    fn from(src: AuthenticatorError) -> Status {
        Status::unauthenticated(src.to_string())
    }
}

impl<T> From<AuthenticatorError> for Result<T, Status> {
    fn from(src: AuthenticatorError) -> Result<T, Status> {
        Err(Status::unauthenticated(src.to_string()))
    }
}

/// Interface for performing an authentication using `BasicCredentials`,
/// resulting in a String username or an error.
pub trait Authenticator: Send + Sync {
    /// Attempt to authenticate a user given their credentials
    fn authenticate(
        &self,
        maybe_credentials: Option<BasicCredentials>,
    ) -> Result<String, AuthenticatorError>;

    /// Attempt to authenticate a user given their MetadataMap
    ///
    /// By default this extracts the BasicCredentials from the MetadataMap
    fn authenticate_metadata(&self, metadata: &MetadataMap) -> Result<String, AuthenticatorError> {
        let creds = metadata
            .get("authorization")
            .map(|value| BasicCredentials::try_from(value.as_bytes()))
            .transpose()?;

        self.authenticate(creds)
    }

    /// Attempt to authenticate a user given a tonic Request
    ///
    /// By default this extracts the request metadata and calls
    /// authenticate_metadata
    fn authenticate_request<T>(&self, request: &Request<T>) -> Result<String, AuthenticatorError> {
        self.authenticate_metadata(request.metadata())
    }
}

/// Standard username/password credentials.
#[derive(Clone, Default)]
pub struct BasicCredentials {
    username: String,
    password: String,
}

/// Errors that can occur when parsing an authorization header
#[derive(Display, Debug, PartialEq)]
pub enum AuthorizationHeaderError {
    /// Unsupported authorization method
    UnsupportedAuthorizationMethod,

    /// Invalid authorization header
    InvalidAuthorizationHeader,

    /// Invalid credentials
    InvalidCredentials,
}

impl BasicCredentials {
    /// Construct a new `BasicCredentials` using provided username and password.
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            username: username.to_owned(),
            password: password.to_owned(),
        }
    }

    /// Try and construct `BasicCredentials` from an HTTP Basic Authorization
    /// header.
    pub fn try_from(header_value: &[u8]) -> Result<Self, AuthorizationHeaderError> {
        let header = str::from_utf8(header_value)
            .map_err(|_| AuthorizationHeaderError::InvalidAuthorizationHeader)?;
        let mut header_parts = header.split(' ');

        if "Basic"
            != header_parts
                .next()
                .ok_or(AuthorizationHeaderError::InvalidAuthorizationHeader)?
        {
            return Err(AuthorizationHeaderError::UnsupportedAuthorizationMethod);
        }

        let base64_value = header_parts
            .next()
            .ok_or(AuthorizationHeaderError::InvalidAuthorizationHeader)?;
        let concatenated_values_bytes = BASE64_ENGINE
            .decode(base64_value)
            .map_err(|_| AuthorizationHeaderError::InvalidAuthorizationHeader)?;
        let concatenated_values = str::from_utf8(&concatenated_values_bytes)
            .map_err(|_| AuthorizationHeaderError::InvalidCredentials)?;
        let mut credential_parts = concatenated_values.splitn(2, ':');

        Ok(Self {
            username: credential_parts
                .next()
                .ok_or(AuthorizationHeaderError::InvalidCredentials)?
                .to_string(),
            password: credential_parts
                .next()
                .ok_or(AuthorizationHeaderError::InvalidCredentials)?
                .to_string(),
        })
    }

    /// Get username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Get password.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Convenience method for constructing an HTTP Authorization header based
    /// on the username and password stored in this object.
    pub fn authorization_header(&self) -> String {
        format!(
            "Basic {}",
            BASE64_ENGINE.encode(format!("{}:{}", self.username, self.password))
        )
    }
}
