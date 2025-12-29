// Copyright (c) 2024 Botho Foundation

//! Implementation of the GRPC Health Checking Protocol with Botho-specific extensions.

use crate::{
    grpc_health_v1::{
        health_server::{Health, HealthServer},
        HealthCheckRequest, HealthCheckResponse, PingRequest, PingResponse,
    },
    rpc_logger,
};
use bt_common::logger::{log, Logger};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

pub use crate::grpc_health_v1::health_check_response::ServingStatus as HealthCheckStatus;

/// Callback function that receives a service name and returns its health status.
pub type ServiceHealthCheckCallback = Arc<dyn Fn(&str) -> HealthCheckStatus + Sync + Send>;

/// A service that serves the gRPC health API
#[derive(Clone)]
pub struct HealthService {
    service_health_check_callback: Option<ServiceHealthCheckCallback>,
    logger: Logger,
}

impl HealthService {
    /// Create a new health service with optional health check callback logic
    pub fn new(
        service_health_check_callback: Option<ServiceHealthCheckCallback>,
        logger: Logger,
    ) -> Self {
        Self {
            service_health_check_callback,
            logger,
        }
    }

    /// Convert into a tonic gRPC service
    pub fn into_service(self) -> HealthServer<Self> {
        HealthServer::new(self)
    }
}

#[tonic::async_trait]
impl Health for HealthService {
    async fn check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        let req = request.into_inner();

        let status = match &self.service_health_check_callback {
            None => HealthCheckStatus::Serving,
            Some(callback) => callback(&req.service),
        };

        log::trace!(logger, "Health check for '{}': {:?}", req.service, status);

        Ok(Response::new(HealthCheckResponse {
            status: status.into(),
        }))
    }

    async fn ping(
        &self,
        request: Request<PingRequest>,
    ) -> Result<Response<PingResponse>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        let req = request.into_inner();

        log::trace!(logger, "Ping with {} bytes", req.data.len());

        Ok(Response::new(PingResponse { data: req.data }))
    }

    type WatchStream = ReceiverStream<Result<HealthCheckResponse, Status>>;

    async fn watch(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        Err(Status::unimplemented("Watch is not implemented"))
    }
}

/// A "global" readiness indicator for services with an initial startup period.
#[derive(Default, Clone)]
pub struct ReadinessIndicator {
    is_ready: Arc<AtomicBool>,
}

impl ReadinessIndicator {
    /// Set the status to ready
    pub fn set_ready(&self) {
        self.is_ready.store(true, Ordering::SeqCst);
    }

    /// Set the status to unready
    pub fn set_unready(&self) {
        self.is_ready.store(false, Ordering::SeqCst);
    }

    /// Check the status
    pub fn ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }
}

impl From<ReadinessIndicator> for ServiceHealthCheckCallback {
    fn from(src: ReadinessIndicator) -> Self {
        Arc::new(move |_| -> HealthCheckStatus {
            if src.ready() {
                HealthCheckStatus::Serving
            } else {
                HealthCheckStatus::NotServing
            }
        })
    }
}
