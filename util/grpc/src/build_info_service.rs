// Copyright (c) 2018-2022 The Botho Foundation

//! Implementation of the BuildInfoApi service.

use crate::{
    build_info::{create_build_info_api, BuildInfo, BuildInfoApi},
    rpc_logger, send_result, SVC_COUNTERS,
};
use grpcio::{RpcContext, Service, UnarySink};
use bth_common::logger::Logger;

/// A service that exposes the BuildInfo of a service recorded using
/// bth_util_build_info
#[derive(Clone)]
pub struct BuildInfoService {
    logger: Logger,
}

impl BuildInfoService {
    /// Create a new instance of the BuildInfo service
    pub fn new(logger: Logger) -> Self {
        Self { logger }
    }

    /// Convert into a grpcio::Service
    pub fn into_service(self) -> Service {
        create_build_info_api(self)
    }
}

/// Get the BuildInfo object, by reading from the BuildInfo crate
pub fn get_build_info() -> BuildInfo {
    BuildInfo {
        git_commit: ::bth_util_build_info::git_commit().to_owned(),
        profile: ::bth_util_build_info::profile().to_owned(),
        debug: ::bth_util_build_info::debug().to_owned(),
        opt_level: ::bth_util_build_info::opt_level().to_owned(),
        debug_assertions: ::bth_util_build_info::debug_assertions().to_owned(),
        target_arch: ::bth_util_build_info::target_arch().to_owned(),
        target_feature: ::bth_util_build_info::target_feature().to_owned(),
        rustflags: ::bth_util_build_info::rustflags().to_owned(),
        sgx_mode: ::bth_util_build_info::sgx_mode().to_owned(),
    }
}

impl BuildInfoApi for BuildInfoService {
    fn get_build_info(&mut self, ctx: RpcContext, _req: (), sink: UnarySink<BuildInfo>) {
        let _timer = SVC_COUNTERS.req(&ctx);
        let logger = rpc_logger(&ctx, &self.logger);
        send_result(ctx, sink, Ok(get_build_info()), &logger);
    }
}
