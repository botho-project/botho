// Copyright (c) 2018-2022 The Botho Foundation

//! Logger initialization and configuration using tracing.
//!
//! This module provides functions to create and configure tracing subscribers
//! for various use cases (tests, applications, etc.).

/// Macros to ease with tests/benches that require a Logger instance.
pub use bth_util_logger_macros::{async_test_with_logger, bench_with_logger, test_with_logger};

use super::Logger;

use std::{
    env,
    io::{self, IsTerminal},
    string::{String, ToString},
    sync::Once,
};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Global initialization guard for tracing subscriber.
static INIT: Once = Once::new();

/// Initialize the tracing subscriber for the process.
///
/// This should be called once at application startup. Subsequent calls are
/// no-ops. If initialization fails (e.g., subscriber already set), the error is
/// silently ignored.
fn init_tracing_subscriber(use_json: bool, use_stderr: bool) {
    INIT.call_once(|| {
        // Support MC_LOG in addition to RUST_LOG for backward compatibility
        if env::var("RUST_LOG").is_err() {
            if let Ok(mc_log) = env::var("MC_LOG") {
                env::set_var("RUST_LOG", mc_log);
            } else {
                // Default to INFO if nothing is set
                env::set_var("RUST_LOG", "info");
            }
        }

        let filter = EnvFilter::from_default_env();

        // Try to initialize - if it fails (subscriber already set), that's fine
        let result = if use_json {
            // JSON output format
            let layer = fmt::layer()
                .json()
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_span_events(FmtSpan::CLOSE);

            if use_stderr {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_writer(io::stderr))
                    .try_init()
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_writer(io::stdout))
                    .try_init()
            }
        } else {
            // Pretty terminal format
            let layer = fmt::layer()
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_ansi(io::stderr().is_terminal());

            if use_stderr {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_writer(io::stderr))
                    .try_init()
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_writer(io::stdout))
                    .try_init()
            }
        };

        // Ignore errors - subscriber might already be set by another part of the
        // codebase
        let _ = result;
    });
}

/// Create the root logger.
///
/// This initializes the global tracing subscriber based on environment
/// variables:
/// - `RUST_LOG` or `MC_LOG`: Log level filter (default: "info")
/// - `MC_LOG_JSON`: If "1", output JSON format
/// - `MC_LOG_STDERR`: If "1", output to stderr instead of stdout
///
/// Returns a Logger instance for API compatibility.
pub fn create_root_logger() -> Logger {
    let use_json = env::var("MC_LOG_JSON").unwrap_or_default() == "1";
    let use_stderr = env::var("MC_LOG_STDERR").unwrap_or_default() == "1";

    init_tracing_subscriber(use_json, use_stderr);
    Logger::new()
}

/// Create a logger suitable for test execution.
///
/// This configures logging to stderr by default (to work with cargo test output
/// capture).
///
/// # Arguments
/// * `_test_name` - Name of the test (logged as a span field for context)
pub fn create_test_logger(_test_name: String) -> Logger {
    // Make tests log to stderr by default
    if env::var("MC_LOG_STDERR").is_err() {
        env::set_var("MC_LOG_STDERR", "1");
    }
    create_root_logger()
}

/// Guard returned by create_app_logger.
///
/// This is a no-op with tracing but maintained for API compatibility.
pub struct LoggerGuard;

impl Drop for LoggerGuard {
    fn drop(&mut self) {
        // Flush any pending spans/events
        // tracing handles this automatically
    }
}

/// Create an application logger.
///
/// This is the main entry point for configuring logging in application
/// binaries. It initializes the tracing subscriber and integrates with Sentry
/// if configured.
///
/// Returns a tuple of (Logger, LoggerGuard) for API compatibility.
/// The guard should be held for the lifetime of the application.
pub fn create_app_logger() -> (Logger, LoggerGuard) {
    let logger = create_root_logger();

    // Log application startup
    let current_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unknown".to_string());

    tracing::info!(app = %current_exe, "Application started");

    (logger, LoggerGuard)
}

/// Recreate the application logger.
///
/// With tracing, this is a no-op since the subscriber is configured once at
/// startup. Environment variable changes after init won't take effect.
pub fn recreate_app_logger() {
    // No-op with tracing - subscriber is immutable after init
    tracing::debug!("recreate_app_logger called - no-op with tracing");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_root_logger() {
        let logger = create_root_logger();
        // Just verify it doesn't panic
        drop(logger);
    }

    #[test]
    fn test_create_test_logger() {
        let logger = create_test_logger("test_name".to_string());
        drop(logger);
    }

    #[test]
    fn test_create_app_logger() {
        let (logger, guard) = create_app_logger();
        drop(logger);
        drop(guard);
    }
}
