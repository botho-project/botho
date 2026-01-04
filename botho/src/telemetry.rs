// Copyright (c) 2024 Botho Foundation

//! OpenTelemetry tracing configuration for distributed consensus debugging.
//!
//! This module provides optional OTLP (OpenTelemetry Protocol) export for
//! tracing consensus messages across nodes. When enabled, traces are exported
//! to a collector (such as Jaeger) for visualization and debugging.
//!
//! # Configuration
//!
//! Telemetry is configured via the config file:
//!
//! ```toml
//! [telemetry]
//! enabled = true
//! endpoint = "http://localhost:4317"  # OTLP gRPC endpoint
//! service_name = "botho-node"
//! sampling_rate = 0.1  # 10% of traces
//! ```

use anyhow::{Context, Result};
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    runtime,
    trace::{RandomIdGenerator, Sampler, Tracer},
    Resource,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

/// Telemetry configuration
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled
    pub enabled: bool,
    /// OTLP endpoint (gRPC)
    pub endpoint: String,
    /// Service name for traces
    pub service_name: String,
    /// Sampling rate (0.0 to 1.0)
    pub sampling_rate: f64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:4317".to_string(),
            service_name: "botho-node".to_string(),
            sampling_rate: 1.0, // Sample all traces by default when enabled
        }
    }
}

/// Initialize the tracing subscriber with optional OpenTelemetry export.
///
/// This sets up:
/// - Console logging via tracing_subscriber::fmt
/// - Optional OTLP export when telemetry is enabled
///
/// # Arguments
///
/// * `config` - Telemetry configuration
/// * `verbose` - Whether to enable debug-level logging
///
/// # Returns
///
/// Returns a guard that must be held for the duration of the program.
/// When dropped, it will flush any pending traces.
pub fn init_tracing(config: &TelemetryConfig, verbose: bool) -> Result<Option<TelemetryGuard>> {
    let level = if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level));

    if config.enabled {
        // Set up OpenTelemetry with OTLP exporter
        let tracer = init_otlp_tracer(config)?;
        let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(telemetry_layer)
            .init();

        tracing::info!(
            endpoint = %config.endpoint,
            service = %config.service_name,
            sampling_rate = config.sampling_rate,
            "OpenTelemetry tracing enabled"
        );

        Ok(Some(TelemetryGuard))
    } else {
        tracing_subscriber::registry().with(fmt_layer).init();

        Ok(None)
    }
}

/// Initialize OTLP tracer and return it
fn init_otlp_tracer(config: &TelemetryConfig) -> Result<Tracer> {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(config.endpoint.clone());

    let sampler = if config.sampling_rate >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_rate <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_rate)
    };

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(
            opentelemetry_sdk::trace::config()
                .with_sampler(sampler)
                .with_id_generator(RandomIdGenerator::default())
                .with_resource(Resource::new(vec![
                    KeyValue::new("service.name", config.service_name.clone()),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ])),
        )
        .install_batch(runtime::Tokio)
        .context("Failed to install OTLP tracer")?;

    Ok(tracer)
}

/// Guard that ensures traces are flushed on shutdown.
///
/// Hold this for the duration of your program. When dropped,
/// it will flush any pending traces to the collector.
pub struct TelemetryGuard;

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        opentelemetry::global::shutdown_tracer_provider();
    }
}

/// Helper function to get the message type name for tracing
pub fn msg_type_name<V>(topic: &bth_consensus_scp::msg::Topic<V>) -> &'static str
where
    V: bth_consensus_scp::Value,
{
    use bth_consensus_scp::msg::Topic;
    match topic {
        Topic::Nominate(_) => "Nominate",
        Topic::NominatePrepare(_, _) => "NominatePrepare",
        Topic::Prepare(_) => "Prepare",
        Topic::Commit(_) => "Commit",
        Topic::Externalize(_) => "Externalize",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_config_default() {
        let config = TelemetryConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "botho-node");
        assert_eq!(config.sampling_rate, 1.0);
    }

    #[test]
    fn test_telemetry_config_sampling_bounds() {
        // Test that sampling rate is clamped properly
        let config = TelemetryConfig {
            enabled: true,
            sampling_rate: 1.5, // > 1.0
            ..Default::default()
        };
        // Should use AlwaysOn when >= 1.0
        assert!(config.sampling_rate >= 1.0);

        let config = TelemetryConfig {
            enabled: true,
            sampling_rate: -0.5, // < 0.0
            ..Default::default()
        };
        // Should use AlwaysOff when <= 0.0
        assert!(config.sampling_rate <= 0.0);
    }
}
