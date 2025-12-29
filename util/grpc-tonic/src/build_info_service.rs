// Copyright (c) 2024 Botho Foundation

//! Implementation of the BuildInfoApi service.

use crate::{
    build_info::{
        build_info_api_server::{BuildInfoApi, BuildInfoApiServer},
        BuildInfo,
    },
    rpc_logger,
};
use bth_common::logger::{log, Logger};
use tonic::{Request, Response, Status};

/// A service that exposes the BuildInfo of a service
#[derive(Clone)]
pub struct BuildInfoService {
    logger: Logger,
}

impl BuildInfoService {
    /// Create a new instance of the BuildInfo service
    pub fn new(logger: Logger) -> Self {
        Self { logger }
    }

    /// Convert into a tonic gRPC service
    pub fn into_service(self) -> BuildInfoApiServer<Self> {
        BuildInfoApiServer::new(self)
    }
}

/// Get the BuildInfo object from bth_util_build_info
pub fn get_build_info() -> BuildInfo {
    BuildInfo {
        git_commit: bth_util_build_info::git_commit().to_owned(),
        profile: bth_util_build_info::profile().to_owned(),
        debug: bth_util_build_info::debug().to_owned(),
        opt_level: bth_util_build_info::opt_level().to_owned(),
        debug_assertions: bth_util_build_info::debug_assertions().to_owned(),
        target_arch: bth_util_build_info::target_arch().to_owned(),
        target_feature: bth_util_build_info::target_feature().to_owned(),
        rustflags: bth_util_build_info::rustflags().to_owned(),
        sgx_mode: bth_util_build_info::sgx_mode().to_owned(),
    }
}

#[tonic::async_trait]
impl BuildInfoApi for BuildInfoService {
    async fn get_build_info(
        &self,
        request: Request<()>,
    ) -> Result<Response<BuildInfo>, Status> {
        let logger = rpc_logger(&request, &self.logger);
        log::trace!(logger, "GetBuildInfo called");
        Ok(Response::new(get_build_info()))
    }
}
