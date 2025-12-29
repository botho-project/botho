// Copyright (c) 2024 Botho Foundation

//! Tonic-based gRPC utilities for Botho services.

mod admin_service;
mod auth;
mod build_info_service;
mod chain_id;
mod health_service;

pub use admin_service::{AdminService, GetConfigJsonFn};
pub use auth::{
    AnonymousAuthenticator, Authenticator, AuthenticatorError, AuthorizationHeaderError,
    BasicCredentials, TokenAuthenticator, TokenBasicCredentialsGenerator,
    TokenBasicCredentialsGeneratorError, ANONYMOUS_USER,
};
pub use build_info_service::{get_build_info, BuildInfoService};
pub use chain_id::{
    check_chain_id_metadata, check_request_chain_id, CHAIN_ID_GRPC_HEADER, CHAIN_ID_MISMATCH_ERR_MSG,
};
pub use health_service::{
    HealthCheckStatus, HealthService, ReadinessIndicator, ServiceHealthCheckCallback,
};
pub use bth_util_metrics::ServiceMetrics;

// Include the generated protobuf code
pub mod grpc_health_v1 {
    include!(concat!(env!("OUT_DIR"), "/protos-auto-gen/grpc.health.v1.rs"));
}

pub mod build_info {
    include!(concat!(env!("OUT_DIR"), "/protos-auto-gen/build_info.rs"));
}

pub mod admin {
    include!(concat!(env!("OUT_DIR"), "/protos-auto-gen/admin.rs"));
}

use bth_common::logger::{log, Logger};
use std::sync::{atomic::{AtomicU64, Ordering}, LazyLock};
use tonic::{Request, Status};

/// Generates service metrics with service name for tracking
pub static SVC_COUNTERS: LazyLock<ServiceMetrics> = LazyLock::new(ServiceMetrics::default);

// Generate a random seed at startup so that rpc_client_id hashes are not identifying specific
// users by leaking IP addresses.
static RPC_LOGGER_CLIENT_ID_SEED: LazyLock<String> = LazyLock::new(|| {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    std::iter::repeat(())
        .map(|()| rng.sample(rand::distributions::Alphanumeric))
        .take(32)
        .map(char::from)
        .collect()
});

static RPC_LOGGER_REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Create a logger for an RPC request
pub fn rpc_logger<T>(request: &Request<T>, logger: &Logger) -> Logger {
    let remote_addr = request
        .remote_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let hash = bth_common::fast_hash(
        format!("{}{}", *RPC_LOGGER_CLIENT_ID_SEED, remote_addr).as_bytes(),
    );
    let hash_str = hex_fmt::HexFmt(hash).to_string();
    let request_id = RPC_LOGGER_REQUEST_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

    logger.new(slog::o!(
        "remote_addr" => remote_addr,
        "rpc_client_id" => hash_str,
        "rpc_request_id" => request_id
    ))
}

/// Convert an error into a tonic Status for internal errors
pub fn rpc_internal_error<E: std::fmt::Display>(
    context: &str,
    error: E,
    logger: &Logger,
) -> Status {
    log::error!(logger, "{}: {}", context, error);
    Status::internal(format!("{}: {}", context, error))
}

/// Database errors are mapped to "Internal Error" and logged at error level
#[inline]
pub fn rpc_database_err<E: std::fmt::Display>(err: E, logger: &Logger) -> Status {
    log::error!(logger, "Database Error: {}", err);
    Status::internal(format!("Database Error: {}", err))
}

/// Invalid arg is listed at debug level, because it can be triggered by bad
/// clients, and may not indicate an actionable issue with the servers.
#[inline]
pub fn rpc_invalid_arg_error<S: std::fmt::Display, E: std::fmt::Display>(
    context: S,
    err: E,
    logger: &Logger,
) -> Status {
    log::debug!(logger, "{}: {}", context, err);
    Status::invalid_argument(format!("{}: {}", context, err))
}

/// Permissions error is listed at debug level, because it can be triggered by
/// clients in normal operation, and may not indicate an actionable issue with
/// the servers.
#[inline]
pub fn rpc_permissions_error<S: std::fmt::Display, E: std::fmt::Display>(
    context: S,
    err: E,
    logger: &Logger,
) -> Status {
    log::debug!(logger, "{}: {}", context, err);
    Status::permission_denied(format!("{}: {}", context, err))
}

/// Out-of-range error occurs when a client makes a request that is out of
/// bounds.
#[inline]
pub fn rpc_out_of_range_error<S: std::fmt::Display, E: std::fmt::Display>(
    context: S,
    err: E,
    logger: &Logger,
) -> Status {
    log::debug!(logger, "{}: {}", context, err);
    Status::out_of_range(format!("{}: {}", context, err))
}

/// Precondition error occurs when a client makes a request that can't be
/// satisfied for the server's current state.
#[inline]
pub fn rpc_precondition_error<S: std::fmt::Display, E: std::fmt::Display>(
    context: S,
    err: E,
    logger: &Logger,
) -> Status {
    log::info!(logger, "{}: {}", context, err);
    Status::failed_precondition(format!("{}: {}", context, err))
}

/// Unavailable error may be returned if e.g. an rpc call fails but could
/// succeed if it is retried.
#[inline]
pub fn rpc_unavailable_error<S: std::fmt::Display, E: std::fmt::Display>(
    context: S,
    err: E,
    logger: &Logger,
) -> Status {
    log::debug!(logger, "{}: {}", context, err);
    Status::unavailable(format!("{}: {}", context, err))
}

/// Converts a serialization Error to a Status error.
pub fn ser_to_rpc_err(error: bth_util_serial::encode::Error, logger: &Logger) -> Status {
    rpc_internal_error("Serialization", error, logger)
}

/// Converts a deserialization Error to a Status error.
pub fn deser_to_rpc_err(error: bth_util_serial::decode::Error, logger: &Logger) -> Status {
    rpc_internal_error("Deserialization", error, logger)
}

/// Converts an encode Error to a Status error.
pub fn encode_to_rpc_err(error: bth_util_serial::EncodeError, logger: &Logger) -> Status {
    rpc_internal_error("Encode", error, logger)
}

/// Converts a decode Error to a Status error.
pub fn decode_to_rpc_err(error: bth_util_serial::DecodeError, logger: &Logger) -> Status {
    rpc_internal_error("Decode", error, logger)
}
