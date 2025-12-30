// Build script to capture git commit hash at compile time

use std::process::Command;

fn main() {
    // Get git commit hash
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok();

    let git_hash = output
        .as_ref()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout.clone()).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Get short hash
    let git_hash_short = if git_hash.len() >= 7 {
        &git_hash[..7]
    } else {
        &git_hash
    };

    // Check if working directory is dirty
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let git_hash_display = if dirty {
        format!("{}-dirty", git_hash_short)
    } else {
        git_hash_short.to_string()
    };

    // Get build timestamp
    let build_time = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=GIT_HASH_SHORT={}", git_hash_display);
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);

    // Rerun if git HEAD changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
}
