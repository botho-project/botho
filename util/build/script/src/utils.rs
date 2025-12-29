// Copyright (c) 2018-2022 The Botho Foundation

//! Utility methods

use cargo_emit::rerun_if_changed;
use std::{
    collections::HashSet,
    ffi::OsStr,
    path::Path,
    sync::{LazyLock, Mutex},
};
use walkdir::WalkDir;

const DEFAULT_EXTENSIONS: &[&str] = &["c", "cc", "cpp", "h", "hh", "hpp", "rs", "edl", "proto"];
const DEFAULT_FILES: &[&str] = &["Cargo.toml", "Cargo.lock"];

fn build_hash_set(str_contents: &'static [&'static str]) -> HashSet<&'static OsStr> {
    str_contents.iter().map(OsStr::new).collect()
}

static EXTENSION_SET: LazyLock<Mutex<HashSet<&'static OsStr>>> =
    LazyLock::new(|| Mutex::new(build_hash_set(DEFAULT_EXTENSIONS)));
static FILE_SET: LazyLock<Mutex<HashSet<&'static OsStr>>> =
    LazyLock::new(|| Mutex::new(build_hash_set(DEFAULT_FILES)));

/// Adds all the known source files under the given path which match the given
/// extensions and filenames.
pub fn rerun_if_path_changed(path: &Path) {
    let extensions = EXTENSION_SET
        .lock()
        .expect("Could not acquire lock on extensions");
    let files = FILE_SET
        .lock()
        .expect("Could not acquire lock on extensions");

    for entry in WalkDir::new(path).into_iter().flatten() {
        if entry.path().components().any(|c| c.as_os_str() == "target") {
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if extensions.contains(ext) {
                    rerun_if_changed!(entry.path().display());
                    rerun_if_changed!(entry.path().parent().unwrap().display());
                }
            }
            let fname = entry.file_name();
            if files.contains(fname) {
                rerun_if_changed!(entry.path().display());
            }
        }
    }
}
