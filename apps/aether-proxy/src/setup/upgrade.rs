//! Self-upgrade support for `aether-proxy`.
//!
//! Downloads a release from GitHub, verifies the SHA256 checksum, replaces the
//! running binary atomically, and restarts the active managed service when
//! applicable.

use std::path::{Path, PathBuf};

use aether_http::{apply_http_client_config, HttpClientConfig};
use sha2::{Digest, Sha256};

const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_REPO: &str = "fawney19/Aether";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── GitHub API types ─────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: String,
}

// ── Platform detection ───────────────────────────────────────────────────────

fn detect_platform() -> &'static str {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") && cfg!(target_env = "musl") {
        "linux-musl-amd64"
    } else if cfg!(target_os = "linux")
        && cfg!(target_arch = "aarch64")
        && cfg!(target_env = "musl")
    {
        "linux-musl-arm64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "linux-amd64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "macos-amd64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "macos-arm64"
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        "windows-amd64"
    } else {
        // All supported targets are covered above; this is unreachable for
        // any platform we actually build for.
        panic!("unsupported platform: compile-time target not in the supported matrix")
    }
}

// ── GitHub HTTP client ───────────────────────────────────────────────────────

fn build_github_client() -> anyhow::Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();

    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))?,
        );
    }

    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
    );

    Ok(apply_http_client_config(
        reqwest::Client::builder().default_headers(headers),
        &HttpClientConfig {
            request_timeout_ms: Some(300_000),
            user_agent: Some(format!("aether-proxy/{}", CURRENT_VERSION)),
            ..HttpClientConfig::default()
        },
    )
    .build()?)
}

// ── Release fetching ─────────────────────────────────────────────────────────

async fn fetch_release(
    client: &reqwest::Client,
    version: Option<&str>,
) -> anyhow::Result<GithubRelease> {
    match version {
        Some(ver) => {
            // Accept both "proxy-v0.2.0" and bare "0.2.0"
            let tag = if ver.starts_with("proxy-v") {
                ver.to_string()
            } else {
                format!("proxy-v{}", ver)
            };
            let url = format!(
                "{}/repos/{}/releases/tags/{}",
                GITHUB_API_BASE, GITHUB_REPO, tag
            );
            let resp = client.get(&url).send().await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("release '{}' not found (HTTP {}): {}", tag, status, body);
            }
            Ok(resp.json().await?)
        }
        None => {
            // List releases and find the latest proxy-v* tag
            let url = format!(
                "{}/repos/{}/releases?per_page=20",
                GITHUB_API_BASE, GITHUB_REPO
            );
            let resp = client.get(&url).send().await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("failed to list releases (HTTP {}): {}", status, body);
            }
            let releases: Vec<GithubRelease> = resp.json().await?;
            releases
                .into_iter()
                .find(|r| r.tag_name.starts_with("proxy-v"))
                .ok_or_else(|| anyhow::anyhow!("no proxy-v* release found"))
        }
    }
}

// ── Download via GitHub release direct links ─────────────────────────────────

/// Download a release asset via the public direct download URL:
/// `https://github.com/{repo}/releases/download/{tag}/{filename}`
async fn download_release_file(
    client: &reqwest::Client,
    tag: &str,
    filename: &str,
) -> anyhow::Result<Vec<u8>> {
    let url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        GITHUB_REPO, tag, filename
    );
    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "download failed for '{}' (HTTP {})",
            filename,
            resp.status(),
        );
    }
    Ok(resp.bytes().await?.to_vec())
}

fn parse_checksum(sums_text: &str, filename: &str) -> anyhow::Result<String> {
    for line in sums_text.lines() {
        // Format: "<hash>  <filename>" (GNU coreutils convention)
        let mut parts = line.split_ascii_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name == filename || name.ends_with(filename) {
            return Ok(hash.to_lowercase());
        }
    }
    anyhow::bail!("checksum for '{}' not found in SHA256SUMS.txt", filename);
}

async fn download_and_verify(
    client: &reqwest::Client,
    tag: &str,
    platform: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let archive_name = format!("aether-proxy-{}.tar.gz", platform);

    eprintln!("  Downloading {}...", archive_name);
    let (archive_bytes, checksum_bytes) = tokio::try_join!(
        download_release_file(client, tag, &archive_name),
        download_release_file(client, tag, "SHA256SUMS.txt"),
    )?;
    let checksum_text = String::from_utf8(checksum_bytes)?;

    eprintln!(
        "  Downloaded {} ({} bytes)",
        archive_name,
        archive_bytes.len()
    );

    // Verify SHA256
    let expected_hash = parse_checksum(&checksum_text, &archive_name)?;
    let mut hasher = Sha256::new();
    hasher.update(&archive_bytes);
    let actual_hash = hex::encode(hasher.finalize());

    if actual_hash != expected_hash {
        anyhow::bail!(
            "SHA256 mismatch for {}:\n  expected: {}\n  actual:   {}",
            archive_name,
            expected_hash,
            actual_hash
        );
    }
    eprintln!("  SHA256 verified: {}", &actual_hash[..16]);

    extract_binary(&archive_bytes, dest)?;

    Ok(())
}

// ── Archive extraction ───────────────────────────────────────────────────────

fn extract_binary(archive_bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    // Guard against decompression bombs
    const MAX_BINARY_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = Archive::new(decoder);

    let binary_name = if cfg!(target_os = "windows") {
        "aether-proxy.exe"
    } else {
        "aether-proxy"
    };

    for entry in archive.entries()? {
        let mut entry = entry?;
        // Only accept regular files -- reject symlinks to prevent write-through attacks
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path()?;
        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            let size = entry.header().size()?;
            if size > MAX_BINARY_SIZE {
                anyhow::bail!(
                    "binary too large ({} bytes, max {} bytes)",
                    size,
                    MAX_BINARY_SIZE
                );
            }
            let mut file = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut file)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
            }

            return Ok(());
        }
    }

    anyhow::bail!("'{}' not found in archive", binary_name);
}

// ── Atomic binary replacement ────────────────────────────────────────────────

fn atomic_replace(new_binary: &Path) -> anyhow::Result<PathBuf> {
    let current_exe = std::env::current_exe()?.canonicalize()?;
    let backup_path = current_exe.with_extension("bak");

    // Remove stale backup
    let _ = std::fs::remove_file(&backup_path);

    // current -> .bak
    std::fs::rename(&current_exe, &backup_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to backup current binary '{}' -> '{}': {}",
            current_exe.display(),
            backup_path.display(),
            e
        )
    })?;

    // new -> current
    if let Err(e) = std::fs::rename(new_binary, &current_exe) {
        eprintln!("  ERROR: failed to place new binary, rolling back...");
        let _ = std::fs::rename(&backup_path, &current_exe);
        anyhow::bail!(
            "failed to install new binary '{}' -> '{}': {}",
            new_binary.display(),
            current_exe.display(),
            e
        );
    }

    eprintln!("  Binary replaced: {}", current_exe.display());
    Ok(backup_path)
}

// ── Public entry point ───────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum RestartMode {
    BestEffort,
    Required,
}

async fn execute_upgrade(
    version: Option<&str>,
    require_root: bool,
    restart_mode: RestartMode,
) -> anyhow::Result<()> {
    // Resolve exe path once; reuse throughout the function
    let current_exe = std::env::current_exe()?.canonicalize()?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine binary directory"))?;
    let temp_path = exe_dir.join(".aether-proxy.upgrade.tmp");

    if require_root {
        if !super::service::is_root() {
            anyhow::bail!("automatic upgrade requires root privileges");
        }
    } else if !super::service::is_root() {
        // Check write permission to binary directory for manual upgrade mode.
        let test_path = exe_dir.join(".aether-proxy.write-test");
        match std::fs::File::create(&test_path) {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_path);
            }
            Err(_) => {
                anyhow::bail!(
                    "no write access to {}. Use: sudo aether-proxy upgrade",
                    exe_dir.display()
                );
            }
        }
    }

    let platform = detect_platform();
    eprintln!("  Platform: {}", platform);
    eprintln!("  Current version: {}", CURRENT_VERSION);

    let client = build_github_client()?;
    let release = fetch_release(&client, version).await?;
    let target_tag = &release.tag_name;
    let target_semver = target_tag.strip_prefix("proxy-v").unwrap_or(target_tag);

    eprintln!("  Target version: {} ({})", target_tag, release.name);

    if target_semver == CURRENT_VERSION {
        eprintln!(
            "  Already running version {}, nothing to do.",
            CURRENT_VERSION
        );
        return Ok(());
    }

    eprintln!();
    eprintln!("  Upgrading: {} -> {}", CURRENT_VERSION, target_semver);
    eprintln!();

    if let Err(e) = download_and_verify(&client, target_tag, platform, &temp_path).await {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    let backup_path = match atomic_replace(&temp_path) {
        Ok(backup) => backup,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(e);
        }
    };

    match restart_mode {
        RestartMode::BestEffort => {
            // Use best-effort: binary is already replaced, so a restart failure should
            // not abort the whole upgrade -- the user can restart manually.
            if super::service::is_service_active() {
                if super::service::is_root() {
                    eprintln!("  Restarting managed service...");
                    match super::service::restart_active_service() {
                        Ok(()) => eprintln!("  Service restarted."),
                        Err(e) => {
                            eprintln!("  WARNING: failed to restart service: {}", e);
                            eprintln!("  Run manually: sudo aether-proxy restart");
                        }
                    }
                } else {
                    eprintln!("  Managed service is active, but restart requires root.");
                    eprintln!("  Run: sudo aether-proxy restart");
                    eprintln!("  Skipping restart.");
                }
            } else {
                eprintln!("  No active service detected, skipping restart.");
            }
        }
        RestartMode::Required => {
            if !super::service::is_root() {
                anyhow::bail!("automatic upgrade requires root privileges");
            }
            eprintln!("  Restarting managed service...");
            super::service::restart_active_service()?;
            eprintln!("  Service restarted.");
        }
    }

    eprintln!();
    eprintln!("  Upgrade complete!");
    eprintln!(
        "  Backup kept at: {} (will be cleaned up on next upgrade)",
        backup_path.display()
    );
    Ok(())
}

/// `aether-proxy upgrade [version]` -- self-upgrade from GitHub releases.
pub async fn cmd_upgrade(version: Option<String>) -> anyhow::Result<()> {
    execute_upgrade(version.as_deref(), false, RestartMode::BestEffort).await
}

/// Perform automatic upgrade to a specific version.
///
/// This path is used for server-pushed upgrades: it requires root and expects
/// the currently active managed service to restart successfully.
pub async fn perform_upgrade(version: &str) -> anyhow::Result<()> {
    execute_upgrade(Some(version), true, RestartMode::Required).await
}
