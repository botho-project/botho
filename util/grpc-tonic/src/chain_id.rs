// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

//! Chain ID validation for tonic gRPC requests.

use tonic::{metadata::MetadataMap, Request, Status};

/// The string used for the chain id gRPC header
/// Note that a corresponding HTTP header is defined by the go-grpc-gateway
/// code: Chain-Id
pub const CHAIN_ID_GRPC_HEADER: &str = "chain-id";

/// The error message used when a chain id mismatch occurs
pub const CHAIN_ID_MISMATCH_ERR_MSG: &str = "chain-id mismatch:";

/// Test the chain id of a request metadata against the value on the server side.
/// This does nothing if the client does not supply a chain-id header.
pub fn check_chain_id_metadata(
    server_chain_id: &str,
    metadata: &MetadataMap,
) -> Result<(), Status> {
    if let Some(client_chain_id) = metadata.get(CHAIN_ID_GRPC_HEADER) {
        let client_chain_id_str = client_chain_id
            .to_str()
            .map_err(|_| Status::failed_precondition("Invalid chain-id header encoding"))?;
        if client_chain_id_str != server_chain_id {
            return Err(Status::failed_precondition(format!(
                "{} '{}'",
                CHAIN_ID_MISMATCH_ERR_MSG, server_chain_id
            )));
        }
    }
    Ok(())
}

/// Test the chain id of a tonic request against the value on the server side.
/// This does nothing if the client does not supply a chain-id header.
pub fn check_request_chain_id<T>(server_chain_id: &str, request: &Request<T>) -> Result<(), Status> {
    check_chain_id_metadata(server_chain_id, request.metadata())
}
