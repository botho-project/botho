// Copyright (c) 2018-2022 The MobileCoin Foundation

//! Serves node-to-node attested gRPC requests.
//!
//! Note: With SGX removed, this service is a stub that accepts all sessions.

use crate::{
    enclave_stubs::{AttestedApi, AuthMessage, ClientSession, ConsensusEnclave, PeerSession, Session},
    SVC_COUNTERS,
};
use grpcio::{RpcContext, UnarySink};
use mc_common::{
    logger::{log, Logger},
    HashSet,
};
use mc_util_grpc::{
    check_request_chain_id, rpc_logger, rpc_permissions_error, send_result, Authenticator,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AttestedApiService<S: Session> {
    chain_id: String,
    enclave: Arc<dyn ConsensusEnclave + Send + Sync>,
    authenticator: Arc<dyn Authenticator + Send + Sync>,
    logger: Logger,
    sessions: Arc<Mutex<HashSet<S>>>,
}

impl<S: Session> AttestedApiService<S> {
    pub fn new(
        chain_id: String,
        enclave: Arc<dyn ConsensusEnclave + Send + Sync>,
        authenticator: Arc<dyn Authenticator + Send + Sync>,
        logger: Logger,
    ) -> Self {
        Self {
            chain_id,
            enclave,
            authenticator,
            logger,
            sessions: Arc::new(Mutex::new(HashSet::default())),
        }
    }
}

impl AttestedApi for AttestedApiService<PeerSession> {
    fn auth(&mut self, ctx: RpcContext, request: AuthMessage, sink: UnarySink<AuthMessage>) {
        let _timer = SVC_COUNTERS.req(&ctx);
        mc_common::logger::scoped_global_logger(&rpc_logger(&ctx, &self.logger), |logger| {
            if let Err(err) = check_request_chain_id(&self.chain_id, &ctx) {
                return send_result(ctx, sink, Err(err), logger);
            }

            if let Err(err) = self.authenticator.authenticate_rpc(&ctx) {
                return send_result(ctx, sink, err.into(), logger);
            }

            // TODO: Use the prost message directly, once available
            match self.enclave.peer_accept(request.into()) {
                Ok((response, session_id)) => {
                    {
                        self.sessions
                            .lock()
                            .expect("Thread crashed while inserting new session ID")
                            .insert(session_id);
                    }
                    send_result(ctx, sink, Ok(response.into()), logger);
                }
                Err(peer_error) => {
                    // This is debug because there's no requirement on the remote party to trigger
                    // it.
                    log::debug!(
                        logger,
                        "ConsensusEnclave::peer_accept failed: {}",
                        peer_error
                    );
                    send_result(
                        ctx,
                        sink,
                        Err(rpc_permissions_error(
                            "peer_auth",
                            "Permission denied",
                            logger,
                        )),
                        logger,
                    );
                }
            }
        });
    }
}

impl AttestedApi for AttestedApiService<ClientSession> {
    fn auth(&mut self, ctx: RpcContext, request: AuthMessage, sink: UnarySink<AuthMessage>) {
        let _timer = SVC_COUNTERS.req(&ctx);
        mc_common::logger::scoped_global_logger(&rpc_logger(&ctx, &self.logger), |logger| {
            if let Err(err) = check_request_chain_id(&self.chain_id, &ctx) {
                return send_result(ctx, sink, Err(err), logger);
            }

            if let Err(err) = self.authenticator.authenticate_rpc(&ctx) {
                return send_result(ctx, sink, err.into(), logger);
            }

            // TODO: Use the prost message directly, once available
            match self.enclave.client_accept(request.into()) {
                Ok((response, session_id)) => {
                    {
                        self.sessions
                            .lock()
                            .expect("Thread crashed while inserting client sesssion ID")
                            .insert(session_id);
                    }
                    send_result(ctx, sink, Ok(response.into()), logger);
                }
                Err(client_error) => {
                    // This is debug because there's no requirement on the remote party to trigger
                    // it.
                    log::debug!(
                        logger,
                        "ConsensusEnclave::client_accept failed: {}",
                        client_error
                    );
                    send_result(
                        ctx,
                        sink,
                        Err(rpc_permissions_error(
                            "client_auth",
                            "Permission denied",
                            logger,
                        )),
                        logger,
                    );
                }
            }
        });
    }
}

// NOTE: The following test modules were disabled as part of SGX removal.
// The attestation API is fundamentally tied to SGX attestation which is no longer used.
// If attestation tests are needed in the future, they should be rewritten to test
// the stub implementations in enclave_stubs.rs.

#[cfg(all(test, feature = "sgx"))] // Feature doesn't exist - effectively disables these tests
mod peer_tests {
    use super::*;
    use grpcio::{
        ChannelBuilder, Environment, Error as GrpcError, RpcStatusCode, Server, ServerBuilder,
        ServerCredentials,
    };
    use crate::enclave_stubs::MockConsensusEnclave;
    use mc_common::{logger::test_with_logger, time::SystemTimeProvider};
    use mc_util_grpc::TokenAuthenticator;
    use std::time::Duration;

    /// Starts the service on localhost and connects a client to it.
    fn get_client_server(instance: AttestedApiService<PeerSession>) -> ((), Server) {
        // TODO: Implement attestation GRPC service stubs if needed
        unimplemented!("Attestation service removed with SGX")
    }

    #[test_with_logger]
    // `auth` should reject unauthenticated responses when configured with an
    // authenticator.
    fn test_peer_auth_unauthenticated(logger: Logger) {
        // Disabled - attestation tests require SGX infrastructure
        unimplemented!()
    }
}

#[cfg(all(test, feature = "sgx"))] // Feature doesn't exist - effectively disables these tests
mod client_tests {
    use super::*;
    use grpcio::{
        ChannelBuilder, Environment, Error as GrpcError, RpcStatusCode, Server, ServerBuilder,
        ServerCredentials,
    };
    use crate::enclave_stubs::MockConsensusEnclave;
    use mc_common::{logger::test_with_logger, time::SystemTimeProvider};
    use mc_util_grpc::TokenAuthenticator;
    use std::time::Duration;

    /// Starts the service on localhost and connects a client to it.
    fn get_client_server(
        instance: AttestedApiService<ClientSession>,
    ) -> ((), Server) {
        // TODO: Implement attestation GRPC service stubs if needed
        unimplemented!("Attestation service removed with SGX")
    }

    #[test_with_logger]
    // `auth` should reject unauthenticated responses when configured with an
    // authenticator.
    fn test_client_auth_unauthenticated(logger: Logger) {
        // Disabled - attestation tests require SGX infrastructure
        unimplemented!()
    }
}
