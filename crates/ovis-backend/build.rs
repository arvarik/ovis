//! Stamps build metadata into the binary for `GET /api/v1/system/version`.
//!
//! Everything here degrades to `"unknown"` rather than failing the build: a
//! source tarball with no git metadata must still compile.

use std::process::Command;

fn main() {
    // rust-embed reads ../../ui/dist at compile time; rebuild when it changes.
    println!("cargo:rerun-if-changed=../../ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OVIS_GIT_SHA");

    let git_sha = std::env::var("OVIS_GIT_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| capture("git", &["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = capture("git", &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let git_sha = if dirty && git_sha != "unknown" {
        format!("{git_sha}-dirty")
    } else {
        git_sha
    };

    let rustc = capture(
        &std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string()),
        &["--version"],
    )
    .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=OVIS_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=OVIS_RUSTC_VERSION={rustc}");
    println!(
        "cargo:rustc-env=OVIS_BUILT_AT={}",
        chrono::Utc::now().to_rfc3339()
    );
    println!(
        "cargo:rustc-env=OVIS_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string())
    );
}

fn capture(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().to_string())
}
