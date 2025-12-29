// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation

use bt_util_build_script::Environment;

fn main() {
    let env = Environment::default();

    let proto_dir = env.dir().join("proto");
    let proto_str = proto_dir
        .as_os_str()
        .to_str()
        .expect("Invalid UTF-8 in proto dir");
    cargo_emit::pair!("PROTOS_PATH", "{}", proto_str);

    // Start with our local proto directory (which now includes attest.proto stub)
    let mut all_proto_dirs = vec![proto_str];

    let api_proto_path = env
        .depvar("BT_API_PROTOS_PATH")
        .expect("Could not read api's protos path")
        .to_owned();
    all_proto_dirs.extend(api_proto_path.split(':').collect::<Vec<&str>>());

    bt_util_build_grpc_tonic::compile_protos_with_config(
        all_proto_dirs.as_slice(),
        &[
            "attest.proto",
            "consensus_client.proto",
            "consensus_common.proto",
            "consensus_config.proto",
            "consensus_peer.proto",
        ],
        |builder| {
            builder
                // Use types from mc-api for external.proto types
                .extern_path(".external", "::bt_api::external")
                .extern_path(".blockchain", "::bt_api::blockchain")
        },
    );
}
