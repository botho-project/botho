// Copyright (c) 2024 Botho Foundation

//! Tonic-based gRPC code generation helper.
//!
//! This crate provides utilities for compiling Protocol Buffer definitions
//! into Rust code using tonic-build, generating both client and server stubs.

use bt_util_build_script::Environment;
use std::{ffi::OsStr, fs, path::PathBuf};

/// Compile protobuf files into Rust code using tonic-build, and generate a
/// mod.rs that references all the generated modules.
///
/// # Arguments
/// * `proto_dirs` - Directories to search for proto imports
/// * `proto_files` - Proto files to compile
pub fn compile_protos_and_generate_mod_rs(proto_dirs: &[&str], proto_files: &[&str]) {
    let env = Environment::default();

    // Output directory for generated code.
    let output_destination = env.out_dir().join("protos-auto-gen");

    // If the proto files change, we need to re-run.
    for dir in proto_dirs.iter() {
        bt_util_build_script::rerun_if_path_changed(&PathBuf::from(dir));
    }

    // Delete old code and create output directory.
    let _ = fs::remove_dir_all(&output_destination);
    fs::create_dir_all(&output_destination).expect("failed creating output destination");

    // Configure and run tonic-build
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(&output_destination)
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile(proto_files, proto_dirs)
        .expect("Failed to compile gRPC definitions!");

    // Generate the mod.rs file that includes all the auto-generated code.
    generate_mod_rs(&output_destination);
}

/// Compile protobuf files with custom configuration.
///
/// # Arguments
/// * `proto_dirs` - Directories to search for proto imports
/// * `proto_files` - Proto files to compile
/// * `configure` - Closure to customize the builder
pub fn compile_protos_with_config<F>(proto_dirs: &[&str], proto_files: &[&str], configure: F)
where
    F: FnOnce(tonic_build::Builder) -> tonic_build::Builder,
{
    let env = Environment::default();

    // Output directory for generated code.
    let output_destination = env.out_dir().join("protos-auto-gen");

    // If the proto files change, we need to re-run.
    for dir in proto_dirs.iter() {
        bt_util_build_script::rerun_if_path_changed(&PathBuf::from(dir));
    }

    // Delete old code and create output directory.
    let _ = fs::remove_dir_all(&output_destination);
    fs::create_dir_all(&output_destination).expect("failed creating output destination");

    // Configure and run tonic-build with custom configuration
    let builder = tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(&output_destination)
        .protoc_arg("--experimental_allow_proto3_optional");

    configure(builder)
        .compile(proto_files, proto_dirs)
        .expect("Failed to compile gRPC definitions!");

    // Generate the mod.rs file that includes all the auto-generated code.
    generate_mod_rs(&output_destination);
}

/// Generate a mod.rs file that includes all generated modules.
fn generate_mod_rs(output_dir: &std::path::Path) {
    let mod_file_contents = fs::read_dir(output_dir)
        .expect("failed reading output directory")
        .filter_map(|res| res.map(|e| e.path()).ok())
        .filter_map(|path| {
            if path.extension() == Some(OsStr::new("rs")) && path.file_stem() != Some(OsStr::new("mod")) {
                Some(format!(
                    "#[path = \"{}\"]\npub mod {};",
                    path.file_name().unwrap().to_str().unwrap(),
                    path.file_stem()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .replace('.', "_"),
                ))
            } else {
                None
            }
        })
        .collect::<Vec<String>>()
        .join("\n");

    let mod_file_path = output_dir.join("mod.rs");

    if fs::read_to_string(&mod_file_path).ok().as_ref() != Some(&mod_file_contents) {
        fs::write(&mod_file_path, &mod_file_contents).expect("Failed writing mod.rs");
    }
}
