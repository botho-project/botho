// Copyright (c) 2024 Botho Foundation

fn main() {
    bt_util_build_grpc_tonic::compile_protos_and_generate_mod_rs(
        &["proto"],
        &[
            "proto/health_api.proto",
            "proto/build_info.proto",
            "proto/admin.proto",
        ],
    );
}
