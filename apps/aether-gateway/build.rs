use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=AETHER_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=AETHER_BUILD_TYPE");
    println!("cargo:rerun-if-env-changed=AETHER_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());
    let version = env::var("AETHER_BUILD_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("AETHER_VERSION")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            env::var("GITHUB_REF_NAME")
                .ok()
                .filter(|value| value.trim().starts_with('v'))
        })
        .or_else(git_describe_version)
        .map(|value| normalize_version(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(package_version);

    println!("cargo:rustc-env=AETHER_BUILD_VERSION={version}");

    let build_type = env::var("AETHER_BUILD_TYPE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "source".to_string());
    println!("cargo:rustc-env=AETHER_BUILD_TYPE={build_type}");
}

fn git_describe_version() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

fn normalize_version(value: &str) -> String {
    value
        .trim()
        .strip_prefix('v')
        .unwrap_or(value.trim())
        .to_string()
}
