// Copyright (c) 2018-2022 The Botho Foundation

//! Botho Logging.
//!
//! This module provides logging utilities using the tracing framework.
//! Configuration is controlled via the RUST_LOG environment variable.
//!
//! # Migration from slog
//!
//! This module previously used slog. The following changes apply:
//! - `Logger` is now a lightweight wrapper (no instance needed for logging)
//! - Use `tracing::{info, debug, warn, error, trace}` macros directly
//! - Log levels are configured via RUST_LOG environment variable

/// Re-export tracing log macros.
///
/// These macros provide slog-compatible syntax for logging.
/// The logger parameter is accepted but ignored (tracing is global).
pub mod log {
    /// Log at info level. Logger parameter is ignored (for compatibility).
    #[macro_export]
    macro_rules! log_info {
        ($logger:expr, $($arg:tt)*) => {
            tracing::info!($($arg)*)
        };
    }

    /// Log at debug level. Logger parameter is ignored (for compatibility).
    #[macro_export]
    macro_rules! log_debug {
        ($logger:expr, $($arg:tt)*) => {
            tracing::debug!($($arg)*)
        };
    }

    /// Log at warn level. Logger parameter is ignored (for compatibility).
    #[macro_export]
    macro_rules! log_warn {
        ($logger:expr, $($arg:tt)*) => {
            tracing::warn!($($arg)*)
        };
    }

    /// Log at error level. Logger parameter is ignored (for compatibility).
    #[macro_export]
    macro_rules! log_error {
        ($logger:expr, $($arg:tt)*) => {
            tracing::error!($($arg)*)
        };
    }

    /// Log at trace level. Logger parameter is ignored (for compatibility).
    #[macro_export]
    macro_rules! log_trace {
        ($logger:expr, $($arg:tt)*) => {
            tracing::trace!($($arg)*)
        };
    }

    /// Log at critical level (maps to error). Logger parameter is ignored.
    #[macro_export]
    macro_rules! log_crit {
        ($logger:expr, $($arg:tt)*) => {
            tracing::error!($($arg)*)
        };
    }

    pub use log_crit as crit;
    pub use log_debug as debug;
    pub use log_error as error;
    pub use log_info as info;
    pub use log_trace as trace;
    pub use log_warn as warn;
}

/// Compatibility type for slog's OwnedKV.
///
/// With tracing, key-value context is handled via spans and fields.
/// This type exists for API compatibility.
pub struct OwnedKV;

/// Compatibility macro for slog's o!() key-value builder.
///
/// With tracing, this returns an empty OwnedKV. Context should be
/// added via tracing spans instead.
#[macro_export]
macro_rules! slog_o {
    ($($key:expr => $value:expr),* $(,)?) => {
        $crate::logger::OwnedKV
    };
}

pub use slog_o as o;

/// Logger type for backward compatibility.
///
/// In the tracing world, logging is global and doesn't require passing
/// logger instances. This type exists for API compatibility with code
/// that previously required a Logger parameter.
#[derive(Clone, Debug, Default)]
pub struct Logger {
    _private: (),
}

impl Logger {
    /// Create a new Logger instance.
    ///
    /// Note: With tracing, the logger instance is not used for actual logging.
    /// Logging happens through the global subscriber.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Create a child logger with additional context.
    ///
    /// With tracing, this just returns a clone of self. Context should be
    /// added via tracing spans (e.g., `#[instrument]` or `info_span!`).
    pub fn new_with_context(&self, _kv: OwnedKV) -> Self {
        self.clone()
    }
}

/// Allow Logger.new(o!(...)) syntax for compatibility.
///
/// This trait provides the slog-style child logger creation syntax.
pub trait LoggerExt {
    /// Create a child logger with the given key-value context.
    fn new(&self, kv: OwnedKV) -> Logger;
}

impl LoggerExt for Logger {
    fn new(&self, _kv: OwnedKV) -> Logger {
        self.clone()
    }
}

/// Create a logger that discards everything.
///
/// With tracing, this returns a dummy Logger. To actually disable logging,
/// configure the tracing subscriber appropriately or don't initialize one.
pub fn create_null_logger() -> Logger {
    Logger::new()
}

cfg_if::cfg_if! {
    if #[cfg(feature = "log")] {
        /// A global logger accessor for compatibility.
        ///
        /// With tracing, you don't need to access a global logger - just use
        /// tracing macros directly. This module exists for API compatibility.
        pub mod global_log {
            pub use tracing::{debug, error, info, trace, warn};
            /// Critical-level logging (maps to error in tracing)
            pub use tracing::error as crit;
        }

        /// Get a Logger instance.
        ///
        /// With tracing, this just returns a new Logger wrapper.
        /// Actual logging configuration is done through the subscriber.
        #[cfg(feature = "log")]
        pub fn global_logger() -> Logger {
            Logger::new()
        }

        /// Execute a function with a logger in scope.
        ///
        /// With tracing, this simply executes the function - no scope management needed.
        #[cfg(feature = "log")]
        pub fn scoped_global_logger<F, R>(_logger: &Logger, f: F) -> R
        where
            F: FnOnce(&Logger) -> R,
        {
            let logger = Logger::new();
            f(&logger)
        }
    }
}

cfg_if::cfg_if! {
    // Time tracing - only available when std is enabled
    if #[cfg(all(feature = "log", feature = "std"))] {
        use std::{time::Instant, format, string::String};

        /// Simple time measurement utility using tracing spans.
        ///
        /// Note: With tracing, prefer using `#[tracing::instrument]` or
        /// `tracing::info_span!` for timing. This macro exists for compatibility.
        #[macro_export]
        macro_rules! trace_time {
            ($logger:expr, $($arg:tt)+) => {
                let _trace_time = $crate::logger::TraceTime::new(format!($($arg)+));
            }
        }

        /// Helper struct for tracing elapsed time.
        pub struct TraceTime {
            msg: String,
            start: Instant,
        }

        impl TraceTime {
            /// Start a timer with the given message.
            pub fn new(msg: String) -> Self {
                Self {
                    msg,
                    start: Instant::now(),
                }
            }
        }

        impl Drop for TraceTime {
            fn drop(&mut self) {
                let elapsed = self.start.elapsed();
                let time_in_ms = elapsed.as_secs_f64() * 1000.0;

                let time = match time_in_ms as u64 {
                    0..=3000 => format!("{time_in_ms:.2}ms"),
                    3001..=60000 => format!("{:.2}s", time_in_ms / 1000.0),
                    _ => format!("{:.2}m", time_in_ms / 1000.0 / 60.0),
                };

                tracing::trace!(duration_ms = time_in_ms, "{}: took {}", self.msg, time);
            }
        }

        #[cfg(test)]
        mod trace_time_tests {
            use super::*;

            #[test]
            fn basic_trace_time() {
                // Initialize a test subscriber (ignore errors if already initialized)
                let _ = tracing_subscriber::fmt()
                    .with_test_writer()
                    .try_init();

                let logger = Logger::new();

                {
                    trace_time!(logger, "test inner");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                trace_time!(logger, "test global");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

cfg_if::cfg_if! {
    // Loggers
    if #[cfg(all(feature = "log", feature = "loggers"))] {
        mod loggers;
        pub use loggers::*;
    }
}
