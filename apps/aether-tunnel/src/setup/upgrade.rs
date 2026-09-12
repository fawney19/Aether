//! Self-upgrade support for `aether-tunnel`.
//!
//! Downloads a release from GitHub, verifies the SHA256 checksum, replaces the
//! running binary atomically, and restarts the active managed service when
//! applicable.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_http::{apply_http_client_config, read_response_bytes_with_limit, HttpClientConfig};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use sha2::{Digest, Sha256};

const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_REPO: &str = "fawney19/Aether";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_GITHUB_RELEASE_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_GITHUB_ERROR_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_RELEASE_ARCHIVE_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_CHECKSUM_DOWNLOAD_BYTES: usize = 1024 * 1024;

fn summarize_remote_error_body(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    format!(
        "response body redacted (bytes={}, sha256={})",
        body.len(),
        hex::encode(digest)
    )
}

// ── GitHub API types ─────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

fn tunnel_release_semver(tag: &str) -> anyhow::Result<semver::Version> {
    if tag.len() > 160
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        anyhow::bail!("invalid tunnel release tag")
    }
    let version = tag
        .strip_prefix("tunnel-v")
        .or_else(|| tag.strip_prefix("proxy-v"))
        .ok_or_else(|| anyhow::anyhow!("tunnel release tag has an unsupported prefix"))?;
    semver::Version::parse(version)
        .map_err(|_| anyhow::anyhow!("tunnel release tag is not valid semantic versioning"))
}

fn normalize_requested_release_tag(version: &str) -> anyhow::Result<String> {
    let version = version.trim();
    if version.is_empty() {
        anyhow::bail!("upgrade version must not be empty");
    }
    let tag = if version.starts_with("tunnel-v") || version.starts_with("proxy-v") {
        version.to_string()
    } else {
        format!("tunnel-v{version}")
    };
    tunnel_release_semver(&tag)?;
    Ok(tag)
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

fn build_github_api_client() -> anyhow::Result<reqwest::Client> {
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
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(SafeGithubDnsResolver))
            .default_headers(headers),
        &HttpClientConfig {
            request_timeout_ms: Some(300_000),
            user_agent: Some(format!("aether-tunnel/{}", CURRENT_VERSION)),
            ..HttpClientConfig::default()
        },
    )
    .build()?)
}

fn build_github_download_client() -> anyhow::Result<reqwest::Client> {
    Ok(apply_http_client_config(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("too many GitHub release redirects");
                }
                if is_trusted_github_download_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("GitHub release redirected to an untrusted URL")
                }
            }))
            .dns_resolver(Arc::new(SafeGithubDnsResolver)),
        &HttpClientConfig {
            request_timeout_ms: Some(300_000),
            user_agent: Some(format!("aether-tunnel/{}", CURRENT_VERSION)),
            ..HttpClientConfig::default()
        },
    )
    .build()?)
}

#[derive(Debug)]
struct SafeGithubDnsResolver;

impl Resolve for SafeGithubDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        // Transparent DNS interception may map public domains into RFC 2544's
        // 198.18.0.0/15 benchmark range. This resolver is used exclusively for
        // built-in GitHub update hosts, so permit that synthetic range only for
        // the same trusted host set used by redirect validation.
        let allow_benchmarking_ip = is_trusted_github_host(&host);
        Box::pin(async move {
            let addresses = aether_http::lookup_host_with_limits(
                host.as_str(),
                0,
                aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
            )
            .await
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?;
            validate_github_resolved_addrs_with_fake_ip(&addresses, allow_benchmarking_ip)
                .map_err(|message| {
                    Box::new(std::io::Error::other(message))
                        as Box<dyn std::error::Error + Send + Sync>
                })?;
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
fn validate_github_resolved_addrs(addresses: &[SocketAddr]) -> Result<(), &'static str> {
    validate_github_resolved_addrs_with_fake_ip(addresses, false)
}

fn validate_github_resolved_addrs_with_fake_ip(
    addresses: &[SocketAddr],
    allow_benchmarking_ip: bool,
) -> Result<(), &'static str> {
    if addresses.is_empty() {
        return Err("GitHub DNS resolution returned no addresses");
    }
    if addresses.iter().any(|address| {
        aether_http::is_private_or_reserved_ip(address.ip())
            && !(allow_benchmarking_ip && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
    }) {
        return Err("GitHub DNS resolution returned a private or reserved address");
    }
    Ok(())
}

fn is_trusted_github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("api.github.com")
        || host.eq_ignore_ascii_case("objects.githubusercontent.com")
        || host.ends_with(".objects.githubusercontent.com")
        || host.eq_ignore_ascii_case("release-assets.githubusercontent.com")
        || host.ends_with(".release-assets.githubusercontent.com")
}

fn is_trusted_github_download_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("objects.githubusercontent.com")
        || host.ends_with(".objects.githubusercontent.com")
        || host.eq_ignore_ascii_case("release-assets.githubusercontent.com")
        || host.ends_with(".release-assets.githubusercontent.com")
}

fn is_trusted_github_download_url(url: &url::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    is_trusted_github_download_host(host)
}

// ── Release fetching ─────────────────────────────────────────────────────────

async fn fetch_release(
    client: &reqwest::Client,
    version: Option<&str>,
) -> anyhow::Result<GithubRelease> {
    match version {
        Some(ver) => {
            // Accept both "tunnel-v0.2.0" and the legacy "proxy-v0.2.0".
            let tag = normalize_requested_release_tag(ver)?;
            let url = format!(
                "{}/repos/{}/releases/tags/{}",
                GITHUB_API_BASE, GITHUB_REPO, tag
            );
            let resp = client.get(&url).send().await?;
            let status = resp.status();
            let max_bytes = if status.is_success() {
                MAX_GITHUB_RELEASE_METADATA_BYTES
            } else {
                MAX_GITHUB_ERROR_RESPONSE_BYTES
            };
            let body = read_response_bytes_with_limit(resp, max_bytes)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("failed to read GitHub release response: {error}")
                })?;
            if !status.is_success() {
                anyhow::bail!(
                    "release '{}' not found (HTTP {}): {}",
                    tag,
                    status,
                    summarize_remote_error_body(&body)
                );
            }
            let release: GithubRelease = serde_json::from_slice(&body)?;
            if release.draft || release.tag_name != tag {
                anyhow::bail!("GitHub returned a draft or mismatched tunnel release");
            }
            tunnel_release_semver(&release.tag_name)?;
            Ok(release)
        }
        None => {
            // List releases and find the latest tunnel-v* tag
            let url = format!(
                "{}/repos/{}/releases?per_page=20",
                GITHUB_API_BASE, GITHUB_REPO
            );
            let resp = client.get(&url).send().await?;
            let status = resp.status();
            let max_bytes = if status.is_success() {
                MAX_GITHUB_RELEASE_METADATA_BYTES
            } else {
                MAX_GITHUB_ERROR_RESPONSE_BYTES
            };
            let body = read_response_bytes_with_limit(resp, max_bytes)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("failed to read GitHub releases response: {error}")
                })?;
            if !status.is_success() {
                anyhow::bail!(
                    "failed to list releases (HTTP {}): {}",
                    status,
                    summarize_remote_error_body(&body)
                );
            }
            let releases: Vec<GithubRelease> = serde_json::from_slice(&body)?;
            releases
                .into_iter()
                .find(|release| {
                    !release.draft
                        && !release.prerelease
                        && tunnel_release_semver(&release.tag_name).is_ok()
                })
                .ok_or_else(|| anyhow::anyhow!("no tunnel-v* release found"))
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
    max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    tunnel_release_semver(tag)?;
    if filename.is_empty()
        || filename.len() > 160
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        anyhow::bail!("invalid GitHub release asset name");
    }
    let url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        GITHUB_REPO, tag, filename
    );
    let mut resp = client
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
    if resp
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        anyhow::bail!("download for '{}' exceeds {} bytes", filename, max_bytes);
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        append_bounded_download_chunk(&mut bytes, &chunk, max_bytes, filename)?;
    }
    Ok(bytes)
}

fn append_bounded_download_chunk(
    bytes: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    filename: &str,
) -> anyhow::Result<()> {
    if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
        anyhow::bail!("download for '{}' exceeds {} bytes", filename, max_bytes);
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn parse_checksum(sums_text: &str, filename: &str) -> anyhow::Result<String> {
    let mut matches = Vec::new();
    for line in sums_text.lines() {
        // Format: "<hash>  <filename>" (GNU coreutils convention)
        let mut parts = line.split_ascii_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if parts.next().is_some() {
            continue;
        }
        let name = name.strip_prefix('*').unwrap_or(name);
        if name == filename && hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            matches.push(hash.to_ascii_lowercase());
        }
    }
    match matches.as_slice() {
        [hash] => Ok(hash.clone()),
        [] => anyhow::bail!("checksum for '{}' not found in SHA256SUMS.txt", filename),
        _ => anyhow::bail!(
            "SHA256SUMS.txt contains multiple valid entries for '{}'",
            filename
        ),
    }
}

async fn download_and_verify(
    client: &reqwest::Client,
    tag: &str,
    platform: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let archive_name = format!("aether-tunnel-{}.tar.gz", platform);

    eprintln!("  Downloading {}...", archive_name);
    let (archive_bytes, checksum_bytes) = tokio::try_join!(
        download_release_file(
            client,
            tag,
            &archive_name,
            MAX_RELEASE_ARCHIVE_DOWNLOAD_BYTES,
        ),
        download_release_file(client, tag, "SHA256SUMS.txt", MAX_CHECKSUM_DOWNLOAD_BYTES,),
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
        "aether-tunnel.exe"
    } else {
        "aether-tunnel"
    };

    let mut entries = archive.entries()?;
    let mut entry = entries
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("release archive is empty"))?;
    if entry.header().entry_type() != tar::EntryType::Regular {
        anyhow::bail!("release archive entry is not a regular file");
    }
    let path = entry.path()?;
    if path.as_ref() != Path::new(binary_name) {
        anyhow::bail!(
            "release archive must contain only '{}' at its root",
            binary_name
        );
    }
    let size = entry.header().size()?;
    if size == 0 || size > MAX_BINARY_SIZE {
        anyhow::bail!(
            "invalid binary size ({} bytes, expected 1..={} bytes)",
            size,
            MAX_BINARY_SIZE
        );
    }

    let mut binary = Vec::with_capacity(size as usize);
    entry
        .by_ref()
        .take(MAX_BINARY_SIZE + 1)
        .read_to_end(&mut binary)?;
    if binary.len() as u64 != size {
        anyhow::bail!("release archive binary size does not match its header");
    }
    drop(entry);
    if entries.next().transpose()?.is_some() {
        anyhow::bail!("release archive must contain exactly one entry");
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(dest).map_err(|error| {
        anyhow::anyhow!(
            "refusing to overwrite upgrade staging path '{}': {}",
            dest.display(),
            error
        )
    })?;
    let write_result = (|| -> std::io::Result<()> {
        file.write_all(&binary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = write_result {
        remove_upgrade_file_if_regular(dest);
        return Err(error.into());
    }

    Ok(())
}

fn remove_upgrade_file_if_regular(path: &Path) {
    if std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
    {
        let _ = std::fs::remove_file(path);
    }
}

fn validate_upgrade_storage(current_exe: &Path) -> anyhow::Result<()> {
    let current_metadata = std::fs::symlink_metadata(current_exe)?;
    if current_metadata.file_type().is_symlink() || !current_metadata.is_file() {
        anyhow::bail!("current tunnel executable must be a regular file");
    }
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine binary directory"))?;
    let directory_metadata = std::fs::symlink_metadata(exe_dir)?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        anyhow::bail!("tunnel executable directory must be a real directory");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if current_metadata.uid() != effective_uid
            || current_metadata.mode() & 0o7022 != 0
            || current_metadata.mode() & 0o100 == 0
            || current_metadata.nlink() != 1
        {
            anyhow::bail!("current tunnel executable ownership or permissions are unsafe");
        }
        let directory_mode = directory_metadata.mode();
        if directory_metadata.uid() != effective_uid
            || directory_mode & 0o022 != 0
            || ((directory_mode >> 6) & 0o3) != 0o3
        {
            anyhow::bail!("tunnel executable directory ownership or permissions are unsafe");
        }

        // The immediate directory is protected above. Every ancestor must also be
        // controlled by this user or root; shared writable ancestors are accepted
        // only when the sticky bit prevents unrelated users from replacing entries.
        let canonical_exe_dir = std::fs::canonicalize(exe_dir)?;
        let mut ancestor = canonical_exe_dir.parent();
        while let Some(directory) = ancestor {
            let metadata = std::fs::symlink_metadata(directory)?;
            let mode = metadata.mode();
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || (metadata.uid() != effective_uid && metadata.uid() != 0)
                || (mode & 0o022 != 0 && mode & 0o1000 == 0)
            {
                anyhow::bail!(
                    "tunnel executable ancestor '{}' has unsafe ownership or permissions",
                    directory.display()
                );
            }
            ancestor = directory.parent();
        }
    }

    #[cfg(not(unix))]
    anyhow::bail!(
        "safe atomic tunnel self-upgrade is not supported on this platform; reinstall the release manually"
    );

    #[cfg(unix)]
    Ok(())
}

fn probe_upgrade_directory_write(exe_dir: &Path) -> anyhow::Result<()> {
    let probe_path = exe_dir.join(format!(
        ".aether-tunnel.write-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let probe = options.open(&probe_path)?;
    drop(probe);
    std::fs::remove_file(&probe_path)?;
    Ok(())
}

// ── Atomic binary replacement ────────────────────────────────────────────────

fn atomic_replace(new_binary: &Path) -> anyhow::Result<PathBuf> {
    let current_exe = std::env::current_exe()?.canonicalize()?;
    atomic_replace_paths(&current_exe, new_binary)
}

fn atomic_replace_paths(current_exe: &Path, new_binary: &Path) -> anyhow::Result<PathBuf> {
    let current_exe = std::fs::canonicalize(current_exe)?;
    validate_upgrade_storage(&current_exe)?;
    let current_parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine current binary directory"))?;
    let new_parent = new_binary
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine staged binary directory"))?;
    if std::fs::canonicalize(current_parent)? != std::fs::canonicalize(new_parent)? {
        anyhow::bail!("staged and current tunnel binaries must share one directory");
    }

    let new_metadata = std::fs::symlink_metadata(new_binary)?;
    if new_metadata.file_type().is_symlink() || !new_metadata.is_file() {
        anyhow::bail!("staged tunnel upgrade must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if new_metadata.uid() != effective_uid
            || new_metadata.mode() & 0o7022 != 0
            || new_metadata.nlink() != 1
        {
            anyhow::bail!("staged tunnel upgrade ownership or permissions are unsafe");
        }
    }

    let backup_path = current_exe.with_extension("bak");

    #[cfg(unix)]
    {
        let backup_staging = current_parent.join(format!(
            ".aether-tunnel.backup-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::hard_link(&current_exe, &backup_staging).map_err(|error| {
            anyhow::anyhow!(
                "failed to create a no-clobber backup of '{}': {}",
                current_exe.display(),
                error
            )
        })?;
        let prepare_result = std::fs::File::open(&backup_staging)
            .and_then(|file| file.sync_all())
            .and_then(|_| sync_upgrade_directory(current_parent));
        if let Err(error) = prepare_result {
            remove_upgrade_file_if_regular(&backup_staging);
            return Err(anyhow::anyhow!(
                "failed to durably stage the current tunnel binary backup: {error}"
            ));
        }

        if let Err(error) = std::fs::rename(new_binary, &current_exe) {
            remove_upgrade_file_if_regular(&backup_staging);
            anyhow::bail!(
                "failed to atomically install new binary '{}' -> '{}': {}",
                new_binary.display(),
                current_exe.display(),
                error
            );
        }
        if let Err(sync_error) = sync_upgrade_directory(current_parent) {
            let rollback_result = std::fs::rename(&backup_staging, &current_exe)
                .and_then(|_| sync_upgrade_directory(current_parent));
            if let Err(rollback_error) = rollback_result {
                anyhow::bail!(
                    "installed tunnel binary but directory sync failed ({sync_error}); rollback also failed ({rollback_error})"
                );
            }
            anyhow::bail!(
                "tunnel binary replacement was rolled back after directory sync failed: {sync_error}"
            );
        }

        match std::fs::rename(&backup_staging, &backup_path) {
            Ok(()) => {
                if let Err(error) = sync_upgrade_directory(current_parent) {
                    eprintln!(
                        "  WARNING: new binary is durable, but final backup-name sync failed: {}",
                        error
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "  WARNING: could not rotate prior backup ({}); keeping old binary at {}",
                    error,
                    backup_staging.display()
                );
                eprintln!("  Binary replaced: {}", current_exe.display());
                return Ok(backup_staging);
            }
        }

        eprintln!("  Binary replaced: {}", current_exe.display());
        Ok(backup_path)
    }

    #[cfg(not(unix))]
    {
        let _ = (new_binary, backup_path);
        anyhow::bail!(
            "safe atomic tunnel self-upgrade is not supported on this platform; reinstall the release manually"
        )
    }
}

fn sync_upgrade_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn restore_tunnel_backup(backup_path: &Path) -> anyhow::Result<()> {
    let current_exe = std::env::current_exe()?.canonicalize()?;
    restore_tunnel_backup_paths(&current_exe, backup_path)
}

fn restore_tunnel_backup_paths(current_exe: &Path, backup_path: &Path) -> anyhow::Result<()> {
    let current_exe = std::fs::canonicalize(current_exe)?;
    validate_upgrade_storage(&current_exe)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let current_parent = current_exe
            .parent()
            .ok_or_else(|| anyhow::anyhow!("cannot determine current binary directory"))?;
        if backup_path.parent() != Some(current_parent) {
            anyhow::bail!("tunnel backup is outside the executable directory");
        }
        let metadata = std::fs::symlink_metadata(backup_path)?;
        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != effective_uid
            || metadata.mode() & 0o7022 != 0
            || metadata.nlink() != 1
        {
            anyhow::bail!("tunnel backup ownership or permissions are unsafe");
        }
        std::fs::rename(backup_path, &current_exe)?;
        if let Err(error) = sync_upgrade_directory(current_parent) {
            eprintln!(
                "  WARNING: old binary was restored, but directory sync failed: {}",
                error
            );
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (current_exe, backup_path);
        anyhow::bail!("safe tunnel upgrade rollback is not supported on this platform")
    }
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
    if require_root && !super::service::is_root() {
        anyhow::bail!("automatic upgrade requires root privileges");
    }
    if let Err(error) = validate_upgrade_storage(&current_exe) {
        if !super::service::is_root() {
            anyhow::bail!(
                "no safe write access to {}: {}. Use: sudo aether-tunnel upgrade",
                exe_dir.display(),
                error
            );
        }
        return Err(error);
    }
    let temp_path = exe_dir.join(format!(
        ".aether-tunnel.upgrade-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    if !require_root && !super::service::is_root() {
        // Check write permission to binary directory for manual upgrade mode.
        if probe_upgrade_directory_write(exe_dir).is_err() {
            anyhow::bail!(
                "no safe write access to {}. Use: sudo aether-tunnel upgrade",
                exe_dir.display()
            );
        }
    }

    let platform = detect_platform();
    eprintln!("  Platform: {}", platform);
    eprintln!("  Current version: {}", CURRENT_VERSION);

    let api_client = build_github_api_client()?;
    let release = fetch_release(&api_client, version).await?;
    let target_tag = &release.tag_name;
    let target_semver = tunnel_release_semver(target_tag)?;
    let current_semver = semver::Version::parse(CURRENT_VERSION)
        .map_err(|_| anyhow::anyhow!("current tunnel version is not valid semantic versioning"))?;

    eprintln!(
        "  Target version: {} ({})",
        target_tag,
        release.name.as_deref().unwrap_or("unnamed release")
    );

    if target_semver == current_semver {
        eprintln!(
            "  Already running version {}, nothing to do.",
            CURRENT_VERSION
        );
        return Ok(());
    }
    if target_semver < current_semver {
        anyhow::bail!(
            "refusing to downgrade aether-tunnel from {} to {}",
            current_semver,
            target_semver
        );
    }

    eprintln!();
    eprintln!("  Upgrading: {} -> {}", CURRENT_VERSION, target_semver);
    eprintln!();

    let download_client = build_github_download_client()?;
    if let Err(error) =
        download_and_verify(&download_client, target_tag, platform, &temp_path).await
    {
        remove_upgrade_file_if_regular(&temp_path);
        return Err(error);
    }
    let backup_path = match atomic_replace(&temp_path) {
        Ok(backup) => backup,
        Err(e) => {
            remove_upgrade_file_if_regular(&temp_path);
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
                            eprintln!("  Run manually: sudo aether-tunnel restart");
                        }
                    }
                } else {
                    eprintln!("  Managed service is active, but restart requires root.");
                    eprintln!("  Run: sudo aether-tunnel restart");
                    eprintln!("  Skipping restart.");
                }
            } else {
                eprintln!("  No active service detected, skipping restart.");
            }
        }
        RestartMode::Required => {
            eprintln!("  Restarting managed service...");
            match super::service::restart_active_service() {
                Ok(()) => eprintln!("  Service restarted."),
                Err(restart_error) => {
                    eprintln!(
                        "  ERROR: upgraded service did not restart; restoring the previous binary..."
                    );
                    if let Err(rollback_error) = restore_tunnel_backup(&backup_path) {
                        anyhow::bail!(
                            "upgraded service restart failed ({restart_error}); binary rollback also failed ({rollback_error})"
                        );
                    }
                    match super::service::restart_active_service() {
                        Ok(()) => {
                            anyhow::bail!(
                                "upgraded service restart failed and the previous binary was restored successfully: {restart_error}"
                            );
                        }
                        Err(recovery_restart_error) => {
                            anyhow::bail!(
                                "upgraded service restart failed ({restart_error}); previous binary was restored but its restart also failed ({recovery_restart_error})"
                            );
                        }
                    }
                }
            }
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

/// `aether-tunnel upgrade [version]` -- self-upgrade from GitHub releases.
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

#[cfg(test)]
mod tests {
    use super::{
        append_bounded_download_chunk, atomic_replace_paths, extract_binary,
        is_trusted_github_download_url, is_trusted_github_host, normalize_requested_release_tag,
        parse_checksum, probe_upgrade_directory_write, restore_tunnel_backup_paths,
        summarize_remote_error_body, tunnel_release_semver, validate_github_resolved_addrs,
        validate_github_resolved_addrs_with_fake_ip, validate_upgrade_storage,
    };
    use flate2::{write::GzEncoder, Compression};
    use std::net::SocketAddr;
    use std::path::Path;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn remote_error_body_summary_redacts_payload() {
        let body = b"token=do-not-log&message=upstream-secret";
        let summary = summarize_remote_error_body(body);

        assert!(summary.starts_with("response body redacted (bytes="));
        assert!(summary.contains("sha256="));
        assert!(!summary.contains("do-not-log"));
        assert!(!summary.contains("upstream-secret"));
    }

    #[test]
    fn release_redirects_require_trusted_https_hosts() {
        for trusted in [
            "https://github.com/fawney19/Aether/releases/download/tag/archive.tar.gz",
            "https://objects.githubusercontent.com/github-production-release-asset/archive",
            "https://release-assets.githubusercontent.com/github-production-release-asset/archive",
        ] {
            assert!(is_trusted_github_download_url(
                &url::Url::parse(trusted).unwrap()
            ));
        }
        for untrusted in [
            "http://github.com/fawney19/Aether/archive.tar.gz",
            "https://github.com:8443/fawney19/Aether/archive.tar.gz",
            "https://github.com.evil.example/archive.tar.gz",
            "https://user@github.com/archive.tar.gz",
            "https://api.github.com/repos/fawney19/Aether/releases",
            "https://example.com/archive.tar.gz",
        ] {
            assert!(!is_trusted_github_download_url(
                &url::Url::parse(untrusted).unwrap()
            ));
        }
    }

    #[test]
    fn github_dns_rejects_private_or_mixed_answers() {
        let public = "8.8.8.8:443".parse::<SocketAddr>().unwrap();
        let private = "127.0.0.1:443".parse::<SocketAddr>().unwrap();

        assert!(validate_github_resolved_addrs(&[public]).is_ok());
        assert!(validate_github_resolved_addrs(&[private]).is_err());
        assert!(validate_github_resolved_addrs(&[public, private]).is_err());
        assert!(validate_github_resolved_addrs(&[]).is_err());
    }

    #[test]
    fn github_dns_allows_benchmarking_ip_only_for_trusted_hosts() {
        let fake = "198.18.75.234:443".parse::<SocketAddr>().unwrap();
        assert!(validate_github_resolved_addrs_with_fake_ip(&[fake], true).is_ok());
        assert!(validate_github_resolved_addrs_with_fake_ip(
            &[fake, "127.0.0.1:443".parse().unwrap()],
            true,
        )
        .is_err());
        assert!(validate_github_resolved_addrs_with_fake_ip(&[fake], false).is_err());
        assert!(is_trusted_github_host("api.github.com"));
        assert!(is_trusted_github_host("foo.objects.githubusercontent.com"));
        assert!(!is_trusted_github_host("github.com.evil.example"));
    }

    #[test]
    fn release_download_bytes_are_bounded_without_trusting_content_length() {
        let mut bytes = Vec::new();
        append_bounded_download_chunk(&mut bytes, b"1234", 8, "archive")
            .expect("first chunk should fit");
        append_bounded_download_chunk(&mut bytes, b"5678", 8, "archive")
            .expect("exact limit should fit");
        assert_eq!(bytes, b"12345678");
        assert!(append_bounded_download_chunk(&mut bytes, b"9", 8, "archive").is_err());
        assert_eq!(bytes, b"12345678");
    }

    fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, entry_type, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*entry_type);
            header.set_mode(0o755);
            header.set_size(body.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, path, *body)
                .expect("test archive entry should append");
        }
        builder
            .into_inner()
            .expect("test archive should finish")
            .finish()
            .expect("test gzip should finish")
    }

    #[test]
    fn checksum_requires_one_exact_valid_filename() {
        let filename = "aether-tunnel-linux-amd64.tar.gz";
        assert_eq!(
            parse_checksum(&format!("{HASH_A}  {filename}\n"), filename)
                .expect("exact checksum should parse"),
            HASH_A
        );
        assert!(parse_checksum(&format!("{HASH_A}  nested/{filename}\n"), filename).is_err());
        assert!(parse_checksum(
            &format!("{HASH_A}  {filename}\n{HASH_B} *{filename}\n"),
            filename
        )
        .is_err());
        assert!(parse_checksum(&format!("not-a-hash  {filename}\n"), filename).is_err());
    }

    #[test]
    fn release_tags_require_bounded_semver_without_url_metacharacters() {
        assert_eq!(
            normalize_requested_release_tag("0.3.17").unwrap(),
            "tunnel-v0.3.17"
        );
        assert_eq!(
            normalize_requested_release_tag("proxy-v0.3.17-rc.1").unwrap(),
            "proxy-v0.3.17-rc.1"
        );
        assert!(tunnel_release_semver("tunnel-v1.2.3+build.7").is_ok());
        for invalid in [
            "",
            "latest",
            "tunnel-v../other",
            "tunnel-v1.2.3?x=1",
            "tunnel-v1.2",
            "other-v1.2.3",
        ] {
            assert!(normalize_requested_release_tag(invalid).is_err());
        }
    }

    #[test]
    fn extraction_rejects_ambiguous_or_non_root_archives() {
        let root = std::env::temp_dir().join(format!(
            "aether-tunnel-upgrade-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let destination = root.join("candidate");
        let binary_name = if cfg!(target_os = "windows") {
            "aether-tunnel.exe"
        } else {
            "aether-tunnel"
        };

        let nested_name = format!("nested/{binary_name}");
        assert!(extract_binary(
            &archive(&[(&nested_name, tar::EntryType::Regular, b"binary")]),
            &destination
        )
        .is_err());
        assert!(extract_binary(
            &archive(&[
                (binary_name, tar::EntryType::Regular, b"binary"),
                ("extra", tar::EntryType::Regular, b"extra"),
            ]),
            &destination
        )
        .is_err());
        assert!(!destination.exists());

        std::fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn extraction_refuses_to_follow_existing_staging_symlink() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = std::env::temp_dir().join(format!(
                "aether-tunnel-upgrade-symlink-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).expect("test directory should be created");
            let victim = root.join("victim");
            let destination = root.join("candidate");
            std::fs::write(&victim, b"keep").expect("victim should be written");
            symlink(&victim, &destination).expect("test symlink should be created");
            let binary_name = if cfg!(target_os = "windows") {
                "aether-tunnel.exe"
            } else {
                "aether-tunnel"
            };

            assert!(extract_binary(
                &archive(&[(binary_name, tar::EntryType::Regular, b"replace")]),
                Path::new(&destination)
            )
            .is_err());
            assert_eq!(
                std::fs::read(&victim).expect("victim should remain readable"),
                b"keep"
            );
            std::fs::remove_dir_all(root).expect("test directory should be removed");
        }
    }

    #[cfg(unix)]
    #[test]
    fn extraction_creates_a_private_synced_executable() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "aether-tunnel-upgrade-mode-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("candidate");
        extract_binary(
            &archive(&[("aether-tunnel", tar::EntryType::Regular, b"new-binary")]),
            &destination,
        )
        .expect("valid archive should extract");

        assert_eq!(std::fs::read(&destination).unwrap(), b"new-binary");
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn upgrade_storage_and_write_probe_reject_unsafe_directories_without_fixed_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "aether-tunnel-upgrade-storage-test-{}",
            uuid::Uuid::new_v4()
        ));
        let executable_directory = root.join("bin");
        std::fs::create_dir_all(&executable_directory).unwrap();
        let current = executable_directory.join("aether-tunnel");
        std::fs::write(&current, b"old").unwrap();
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();

        validate_upgrade_storage(&current).expect("private owned storage should pass");
        probe_upgrade_directory_write(&executable_directory)
            .expect("unique write probe should pass");
        assert!(std::fs::read_dir(&executable_directory)
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".aether-tunnel.write-test-")
            }));

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(validate_upgrade_storage(&current).is_err());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(
            &executable_directory,
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(validate_upgrade_storage(&current).is_err());
        std::fs::set_permissions(
            &executable_directory,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_upgrade_replaces_in_one_step_and_preserves_safe_rollback() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "aether-tunnel-atomic-upgrade-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let current = root.join("aether-tunnel");
        let staged = root.join("candidate");
        std::fs::write(&current, b"old-binary").unwrap();
        std::fs::write(&staged, b"new-binary").unwrap();
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();

        let victim = root.join("victim");
        std::fs::write(&victim, b"known-good").unwrap();
        let fixed_backup = current.with_extension("bak");
        symlink(&victim, &fixed_backup).unwrap();

        let backup = atomic_replace_paths(&current, &staged).expect("upgrade should succeed");

        assert_eq!(std::fs::read(&current).unwrap(), b"new-binary");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-binary");
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");
        assert!(!std::fs::symlink_metadata(&backup)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!staged.exists());

        restore_tunnel_backup_paths(&current, &backup).expect("rollback should succeed");
        assert_eq!(std::fs::read(&current).unwrap(), b"old-binary");
        assert!(!backup.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_upgrade_rejects_hardlinked_staging_without_touching_current() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "aether-tunnel-hardlink-upgrade-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let current = root.join("aether-tunnel");
        let staged = root.join("candidate");
        let outside = root.join("outside");
        std::fs::write(&current, b"old-binary").unwrap();
        std::fs::write(&outside, b"new-binary").unwrap();
        std::fs::hard_link(&outside, &staged).unwrap();
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(atomic_replace_paths(&current, &staged).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"old-binary");
        assert_eq!(std::fs::read(&outside).unwrap(), b"new-binary");
        std::fs::remove_dir_all(root).unwrap();
    }
}
