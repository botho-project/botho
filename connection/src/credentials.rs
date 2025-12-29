// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! GRPC credentials support - Simplified for tonic

use displaydoc::Display;
use bth_util_uri::ConnectionUri;
use std::{
    convert::Infallible,
    fmt::{Debug, Display},
};
use tonic::Status;

/// Basic credentials with username and password
#[derive(Clone, Debug)]
pub struct BasicCredentials {
    username: String,
    password: String,
}

impl BasicCredentials {
    /// Create new basic credentials
    pub fn new(username: impl AsRef<str>, password: impl AsRef<str>) -> Self {
        Self {
            username: username.as_ref().to_owned(),
            password: password.as_ref().to_owned(),
        }
    }

    /// Get the username
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Get the password
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Create the authorization header value
    pub fn authorization_header(&self) -> String {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.username, self.password));
        format!("Basic {}", encoded)
    }
}

/// A trait that lets us determine if an error relates to authentication
/// failure.
pub trait AuthenticationError {
    fn is_unauthenticated(&self) -> bool;
}

impl AuthenticationError for Status {
    fn is_unauthenticated(&self) -> bool {
        self.code() == tonic::Code::Unauthenticated
    }
}

/// Error relating to credential providing.
pub trait CredentialsProviderError: Debug + Display + Send + Sync {}
impl<T> CredentialsProviderError for T where T: Debug + Display + Send + Sync {}

/// An interface for providing credentials for a given URI.
pub trait CredentialsProvider: Send + Sync {
    type Error: CredentialsProviderError + 'static;

    /// Get credentials to be used for a GRPC call.
    fn get_credentials(&self) -> Result<Option<BasicCredentials>, Self::Error>;

    /// Clear any cached credentials so that new ones can be generated.
    /// The default implementation is a no-op.
    fn clear(&self) {}
}

/// A credentials provider that has hardcoded user/password credentials.
#[derive(Default)]
pub struct HardcodedCredentialsProvider {
    creds: Option<BasicCredentials>,
}

impl HardcodedCredentialsProvider {
    pub fn new(username: impl AsRef<str>, password: impl AsRef<str>) -> Self {
        Self {
            creds: Some(BasicCredentials::new(username.as_ref(), password.as_ref())),
        }
    }
}

impl<URI: ConnectionUri> From<&URI> for HardcodedCredentialsProvider {
    fn from(src: &URI) -> Self {
        Self::new(src.username(), src.password())
    }
}

impl CredentialsProvider for HardcodedCredentialsProvider {
    type Error = Infallible;

    fn get_credentials(&self) -> Result<Option<BasicCredentials>, Self::Error> {
        Ok(self.creds.clone())
    }
}

/// All possible types of built-in credential providers.
pub enum AnyCredentialsProvider {
    Hardcoded(HardcodedCredentialsProvider),
}

/// Possible error types for built-in credential providers.
#[derive(Debug, Display)]
pub enum AnyCredentialsError {
    /// Infallible
    Infallible,
}

impl From<Infallible> for AnyCredentialsError {
    fn from(_src: Infallible) -> Self {
        Self::Infallible
    }
}

impl std::error::Error for AnyCredentialsError {}

impl CredentialsProvider for AnyCredentialsProvider {
    type Error = AnyCredentialsError;

    fn get_credentials(&self) -> Result<Option<BasicCredentials>, Self::Error> {
        match self {
            Self::Hardcoded(inner) => inner.get_credentials().map_err(Into::into),
        }
    }

    fn clear(&self) {
        match self {
            Self::Hardcoded(inner) => inner.clear(),
        }
    }
}
