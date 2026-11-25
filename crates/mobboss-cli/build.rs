use std::process::Command;

fn main() {
    for name in ["GITHUB_REF_TYPE", "GITHUB_REF_NAME", "MOBBOSS_BUILD_DATE"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    for path in [
        "build.rs",
        "src/build_version.rs",
        "Cargo.toml",
        "../../Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    let tag = (std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag"))
        .then(|| std::env::var("GITHUB_REF_NAME").ok())
        .flatten();
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    let date = std::env::var("MOBBOSS_BUILD_DATE")
        .unwrap_or_else(|_| chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string());
    let version = build_version::format_version(
        tag.as_deref(),
        env!("CARGO_PKG_VERSION"),
        &date,
        commit.as_deref(),
    );
    println!("cargo:rustc-env=MOBBOSS_VERSION={version}");
}

#[path = "src/build_version.rs"]
mod build_version;
