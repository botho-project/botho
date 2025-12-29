// Copyright (c) 2024 Botho Foundation

//! Customizable implementation of the AdminApi service using Tonic.

use crate::{
    admin::{
        admin_api_server::{AdminApi, AdminApiServer},
        GetInfoResponse, GetPrometheusMetricsResponse, SetRustLogRequest,
    },
    build_info_service::get_build_info,
    rpc_logger,
};
use bt_common::logger::{log, Logger};
use prometheus::Encoder;
use std::{env, sync::Arc};
use tonic::{Request, Response, Status};

/// A callback for getting service-specific configuration data.
pub type GetConfigJsonFn = Arc<dyn Fn() -> Result<String, Status> + Sync + Send>;

/// Admin gRPC service.
#[derive(Clone)]
pub struct AdminService {
    /// User-friendly service name (e.g. "Consensus Service").
    name: String,

    /// Unique identifier for the service (e.g. the hostname it is running on).
    id: String,

    /// Optional callback for returning service-specific configuration JSON blob
    get_config_json: Option<GetConfigJsonFn>,

    /// Logger.
    logger: Logger,
}

impl AdminService {
    /// Create a new instance of the admin service
    ///
    /// Arguments:
    /// * name: A name for the server
    /// * id: An id for the server
    /// * get_config_json: An optional callback that describes the current
    ///   configuration of the server as a json object
    /// * logger
    pub fn new(
        name: String,
        id: String,
        get_config_json: Option<GetConfigJsonFn>,
        logger: Logger,
    ) -> Self {
        Self {
            name,
            id,
            get_config_json,
            logger,
        }
    }

    /// Convert into a tonic gRPC service
    pub fn into_service(self) -> AdminApiServer<Self> {
        AdminApiServer::new(self)
    }
}

#[tonic::async_trait]
impl AdminApi for AdminService {
    async fn get_prometheus_metrics(
        &self,
        request: Request<()>,
    ) -> Result<Response<GetPrometheusMetricsResponse>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        log::trace!(logger, "get_prometheus_metrics");

        let metric_families = prometheus::gather();
        let encoder = prometheus::TextEncoder::new();
        let mut buffer = vec![];
        encoder
            .encode(&metric_families, &mut buffer)
            .map_err(|e| Status::internal(format!("Failed to encode metrics: {}", e)))?;

        let metrics = String::from_utf8(buffer)
            .map_err(|e| Status::internal(format!("from_utf8 failed: {}", e)))?;

        Ok(Response::new(GetPrometheusMetricsResponse { metrics }))
    }

    async fn get_info(&self, request: Request<()>) -> Result<Response<GetInfoResponse>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        log::trace!(logger, "get_info");

        let mut build_info_json = String::new();
        bt_util_build_info::write_report(&mut build_info_json)
            .map_err(|e| Status::internal(format!("write_report failed: {}", e)))?;

        let build_info = get_build_info();

        let config_json = if let Some(get_config_json) = self.get_config_json.as_ref() {
            get_config_json()?
        } else {
            String::new()
        };

        let rust_log = env::var("RUST_LOG").unwrap_or_default();

        Ok(Response::new(GetInfoResponse {
            name: self.name.clone(),
            id: self.id.clone(),
            build_info_json,
            build_info: Some(build_info),
            config_json,
            rust_log,
        }))
    }

    async fn set_rust_log(
        &self,
        request: Request<SetRustLogRequest>,
    ) -> Result<Response<()>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        let req = request.into_inner();

        log::info!(logger, "Updating RUST_LOG to '{}'", req.rust_log);
        env::set_var("RUST_LOG", req.rust_log);
        bt_common::logger::recreate_app_logger();

        Ok(Response::new(()))
    }

    async fn test_log_error(&self, request: Request<()>) -> Result<Response<()>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        log::error!(logger, "Test log message from admin interface");

        Ok(Response::new(()))
    }
}
