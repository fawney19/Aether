use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    build_auth_error_response, decrypt_catalog_secret_with_fallbacks,
    decrypt_or_migrate_auth_api_key_secret, encrypt_catalog_secret_with_fallbacks,
    mark_sensitive_response_no_store, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};

const INSTALL_SESSION_TTL_SECS: u64 = 15 * 60;
const INSTALL_SESSION_KEY_PREFIX: &str = "install:session:";
const TUNNEL_INSTALL_SESSION_KEY_PREFIX: &str = "tunnel-install:session:";
const INSTALL_SESSION_ENVELOPE_PREFIX: &str = "aether-install-session-v1:";
const TUNNEL_INSTALL_UNIX_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/fawney19/Aether/refs/heads/main/apps/aether-tunnel/install.sh";
const TUNNEL_INSTALL_POWERSHELL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.ps1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstallTargetCli {
    ClaudeCode,
    CodexCli,
    GeminiCli,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstallTargetSystem {
    Macos,
    Linux,
    Windows,
    Auto,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateApiKeyInstallSessionRequest {
    pub(crate) target_cli: InstallTargetCli,
    pub(crate) target_system: InstallTargetSystem,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredInstallSession {
    install_code: String,
    api_key_id: String,
    api_key_owner_user_id: String,
    api_key_is_standalone: bool,
    api_key_hash: String,
    base_url: String,
    target_cli: InstallTargetCli,
    target_system: InstallTargetSystem,
    expires_at_unix_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredTunnelInstallSession {
    install_code: String,
    aether_url: String,
    management_token_snapshot: aether_data::repository::management_tokens::StoredManagementToken,
    management_token_user_security_version: i64,
    management_token: String,
    node_name: String,
    tunnel_security: String,
    tunnel_encryption_key: String,
    expires_at_unix_secs: u64,
}

pub(super) fn users_me_api_key_install_sessions_path_matches(request_path: &str) -> bool {
    users_me_api_key_install_session_id_from_path(request_path).is_some()
}

fn users_me_api_key_install_session_id_from_path(request_path: &str) -> Option<String> {
    let raw = request_path
        .strip_prefix("/api/users/me/api-keys/")?
        .trim()
        .trim_matches('/');
    let mut segments = raw.split('/').map(str::trim);
    let api_key_id = segments.next()?.to_string();
    let suffix = segments.next()?;
    (suffix == "install-sessions" && segments.next().is_none()).then_some(api_key_id)
}

fn install_code_from_path(request_path: &str) -> Option<(String, bool)> {
    let raw = request_path
        .strip_prefix("/install/")
        .or_else(|| request_path.strip_prefix("/i/"))?
        .trim()
        .trim_matches('/');
    if raw.is_empty() || raw.contains('/') {
        return None;
    }
    let is_powershell = raw.ends_with(".ps1");
    let code = raw.strip_suffix(".ps1").unwrap_or(raw).trim();
    is_valid_install_code(code).then(|| (code.to_string(), is_powershell))
}

fn tunnel_install_code_from_path(request_path: &str) -> Option<(String, bool)> {
    let raw = request_path
        .strip_prefix("/install-tunnel/")?
        .trim()
        .trim_matches('/');
    if raw.is_empty() || raw.contains('/') {
        return None;
    }
    let is_powershell = raw.ends_with(".ps1");
    let code = raw.strip_suffix(".ps1").unwrap_or(raw).trim();
    is_valid_install_code(code).then(|| (code.to_string(), is_powershell))
}

fn is_valid_install_code(code: &str) -> bool {
    code.len() == 24
        && code
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn install_session_runtime_key(code: &str) -> String {
    format!("{INSTALL_SESSION_KEY_PREFIX}sha256:{}", sha256_hex(code))
}

fn tunnel_install_session_runtime_key(code: &str) -> String {
    format!(
        "{TUNNEL_INSTALL_SESSION_KEY_PREFIX}sha256:{}",
        sha256_hex(code)
    )
}

fn generate_install_code() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(24)
        .collect()
}

fn generate_tunnel_encryption_key() -> String {
    use base64::Engine;

    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let mut key = [0_u8; 32];
    key[..16].copy_from_slice(first.as_bytes());
    key[16..].copy_from_slice(second.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(key)
}

fn seal_install_session(state: &AppState, plaintext: &str) -> Option<String> {
    encrypt_catalog_secret_with_fallbacks(state, plaintext)
        .map(|ciphertext| format!("{INSTALL_SESSION_ENVELOPE_PREFIX}{ciphertext}"))
}

fn open_install_session(state: &AppState, stored: &str) -> Option<String> {
    let ciphertext = stored.strip_prefix(INSTALL_SESSION_ENVELOPE_PREFIX)?;
    decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
}

fn unix_secs_now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn api_key_record_is_current_for_install(
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    now_unix_secs: u64,
) -> bool {
    record.is_active
        && record
            .expires_at_unix_secs
            .is_none_or(|expires_at| expires_at > now_unix_secs)
}

fn api_key_record_matches_current_snapshot(
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    snapshot: &aether_data::repository::auth::ResolvedAuthApiKeySnapshot,
    now_unix_secs: u64,
) -> bool {
    snapshot.api_key_id == record.api_key_id
        && snapshot.user_id == record.user_id
        && snapshot.api_key_is_standalone == record.is_standalone
        && snapshot.api_key_is_active == record.is_active
        && snapshot.api_key_expires_at_unix_secs == record.expires_at_unix_secs
        && snapshot.user_is_active
        && !snapshot.user_is_deleted
        && snapshot.api_key_is_active
        && !snapshot.api_key_is_locked
        && snapshot
            .api_key_expires_at_unix_secs
            .is_none_or(|expires_at| expires_at > now_unix_secs)
        && api_key_record_is_current_for_install(record, now_unix_secs)
}

async fn api_key_record_has_current_snapshot(
    state: &AppState,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    now_unix_secs: u64,
) -> Result<bool, crate::GatewayError> {
    let snapshot = state
        .data
        .read_auth_api_key_snapshot_strong(&record.user_id, &record.api_key_id, now_unix_secs)
        .await
        .map_err(|err| crate::GatewayError::Internal(err.to_string()))?;
    Ok(snapshot.is_some_and(|snapshot| {
        api_key_record_matches_current_snapshot(record, &snapshot, now_unix_secs)
    }))
}

fn install_session_matches_api_key_record(
    session: &StoredInstallSession,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    now_unix_secs: u64,
) -> bool {
    record.api_key_id == session.api_key_id
        && record.user_id == session.api_key_owner_user_id
        && record.is_standalone == session.api_key_is_standalone
        && record.key_hash == session.api_key_hash
        && api_key_record_is_current_for_install(record, now_unix_secs)
}

async fn resolve_current_install_session_api_key(
    state: &AppState,
    session: &StoredInstallSession,
    now_unix_secs: u64,
) -> Result<Option<String>, crate::GatewayError> {
    let mut matching = state
        .list_auth_api_key_export_records_by_ids(std::slice::from_ref(&session.api_key_id))
        .await?
        .into_iter()
        .filter(|record| record.api_key_id == session.api_key_id);
    let Some(record) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some()
        || !install_session_matches_api_key_record(session, &record, now_unix_secs)
        || !api_key_record_has_current_snapshot(state, &record, now_unix_secs).await?
    {
        return Ok(None);
    }
    let Some(ciphertext) = record
        .key_encrypted
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let current_api_key = match decrypt_or_migrate_auth_api_key_secret(state, &record).await {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if sha256_hex(&current_api_key) != record.key_hash {
        return Ok(None);
    }
    Ok(Some(current_api_key))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn activate_tunnel_install_management_token(
    state: &AppState,
    session: &StoredTunnelInstallSession,
    now_unix_secs: u64,
) -> Result<bool, crate::GatewayError> {
    if session.management_token_snapshot.permissions != Some(json!(["admin:proxy_nodes:write"]))
        || session
            .management_token_snapshot
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at <= now_unix_secs)
    {
        return Ok(false);
    }

    state
        .activate_management_token_if_matches(&tunnel_install_token_mutation(
            session,
            now_unix_secs,
        ))
        .await
}

fn tunnel_install_token_mutation(
    session: &StoredTunnelInstallSession,
    now_unix_secs: u64,
) -> aether_data::repository::management_tokens::ActivateManagementTokenIfMatches {
    aether_data::repository::management_tokens::ActivateManagementTokenIfMatches {
        expected_token: session.management_token_snapshot.clone(),
        token_hash: sha256_hex(&session.management_token),
        expected_user_security_version: session.management_token_user_security_version,
        now_unix_secs,
    }
}

async fn discard_tunnel_install_session_token(
    state: &AppState,
    session: &StoredTunnelInstallSession,
) {
    let _ = state
        .delete_inactive_management_token_if_matches(&tunnel_install_token_mutation(
            session,
            unix_secs_now(),
        ))
        .await;
}

async fn discard_unused_tunnel_install_management_token(
    state: &AppState,
    record: &aether_data::repository::management_tokens::StoredManagementToken,
    management_token: &str,
    expected_user_security_version: i64,
) {
    let _ = state
        .delete_inactive_management_token_if_matches(
            &aether_data::repository::management_tokens::ActivateManagementTokenIfMatches {
                expected_token: record.clone(),
                token_hash: sha256_hex(management_token),
                expected_user_security_version,
                now_unix_secs: unix_secs_now(),
            },
        )
        .await;
}

pub(crate) fn base_url_from_request(
    headers: &http::HeaderMap,
    request_context: &GatewayPublicRequestContext,
    remote_addr: &std::net::SocketAddr,
) -> String {
    if let Some(value) = std::env::var("AETHER_PUBLIC_BASE_URL")
        .ok()
        .or_else(|| std::env::var("PUBLIC_BASE_URL").ok())
        .and_then(|value| normalize_public_base_url(&value))
    {
        return value;
    }

    base_url_from_request_metadata(
        headers,
        request_context.host_header.as_deref(),
        crate::headers::trusted_proxy_ip(remote_addr.ip()),
    )
}

fn base_url_from_request_metadata(
    headers: &http::HeaderMap,
    host_header: Option<&str>,
    trusted_peer: bool,
) -> String {
    if !trusted_peer {
        return "http://localhost".to_string();
    }

    let host = forwarded_header_last(headers, "x-forwarded-host")
        .or_else(|| {
            host_header
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "localhost".to_string());
    let proto = forwarded_header_last(headers, "x-forwarded-proto")
        .map(|value| value.trim_end_matches(':').to_ascii_lowercase())
        .filter(|value| value == "http" || value == "https")
        .unwrap_or_else(|| "http".to_string());
    normalize_public_base_url(&format!("{proto}://{host}"))
        .unwrap_or_else(|| "http://localhost".to_string())
}

fn forwarded_header_last(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .rfind(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_public_base_url(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    let parsed = url::Url::parse(value).ok()?;
    if !aether_http::is_https_or_loopback_http_url(&parsed)
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(parsed.as_str().trim_end_matches('/').to_string())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn build_tunnel_unix_script(session: &StoredTunnelInstallSession) -> String {
    format!(
        r###"#!/bin/sh
set -eu
export AETHER_TUNNEL_AETHER_URL={aether_url}
export AETHER_TUNNEL_MANAGEMENT_TOKEN={management_token}
export AETHER_TUNNEL_NODE_NAME={node_name}
export AETHER_TUNNEL_SECURITY={tunnel_security}
export AETHER_TUNNEL_ENCRYPTION_KEY={tunnel_encryption_key}

if command -v curl >/dev/null 2>&1; then
  curl -fsSL {script_url} | sh
elif command -v wget >/dev/null 2>&1; then
  wget -qO- {script_url} | sh
else
  printf '%s\n' "[Aether Tunnel] 需要 curl 或 wget 下载安装脚本" >&2
  exit 1
fi
"###,
        aether_url = shell_single_quote(&session.aether_url),
        management_token = shell_single_quote(&session.management_token),
        node_name = shell_single_quote(&session.node_name),
        tunnel_security = shell_single_quote(&session.tunnel_security),
        tunnel_encryption_key = shell_single_quote(&session.tunnel_encryption_key),
        script_url = shell_single_quote(TUNNEL_INSTALL_UNIX_SCRIPT_URL),
    )
}

fn build_tunnel_powershell_script(session: &StoredTunnelInstallSession) -> String {
    format!(
        r###"$ErrorActionPreference = 'Stop'
$env:AETHER_TUNNEL_AETHER_URL = {aether_url}
$env:AETHER_TUNNEL_MANAGEMENT_TOKEN = {management_token}
$env:AETHER_TUNNEL_NODE_NAME = {node_name}
$env:AETHER_TUNNEL_SECURITY = {tunnel_security}
$env:AETHER_TUNNEL_ENCRYPTION_KEY = {tunnel_encryption_key}
irm {script_url} | iex
"###,
        aether_url = powershell_single_quote(&session.aether_url),
        management_token = powershell_single_quote(&session.management_token),
        node_name = powershell_single_quote(&session.node_name),
        tunnel_security = powershell_single_quote(&session.tunnel_security),
        tunnel_encryption_key = powershell_single_quote(&session.tunnel_encryption_key),
        script_url = powershell_single_quote(TUNNEL_INSTALL_POWERSHELL_SCRIPT_URL),
    )
}

fn cli_label(target_cli: InstallTargetCli) -> &'static str {
    match target_cli {
        InstallTargetCli::ClaudeCode => "Claude Code",
        InstallTargetCli::CodexCli => "Codex CLI",
        InstallTargetCli::GeminiCli => "Gemini CLI",
    }
}

fn system_label(target_system: InstallTargetSystem) -> &'static str {
    match target_system {
        InstallTargetSystem::Macos => "macOS",
        InstallTargetSystem::Linux => "Linux",
        InstallTargetSystem::Windows => "Windows",
        InstallTargetSystem::Auto => "Auto",
    }
}

fn npm_package(target_cli: InstallTargetCli) -> &'static str {
    match target_cli {
        InstallTargetCli::ClaudeCode => "@anthropic-ai/claude-code",
        InstallTargetCli::CodexCli => "@openai/codex",
        InstallTargetCli::GeminiCli => "@google/gemini-cli",
    }
}

fn cli_binary(target_cli: InstallTargetCli) -> &'static str {
    match target_cli {
        InstallTargetCli::ClaudeCode => "claude",
        InstallTargetCli::CodexCli => "codex",
        InstallTargetCli::GeminiCli => "gemini",
    }
}

fn build_unix_script(session: &StoredInstallSession, api_key: &str) -> String {
    let target_cli = match session.target_cli {
        InstallTargetCli::ClaudeCode => "claude_code",
        InstallTargetCli::CodexCli => "codex_cli",
        InstallTargetCli::GeminiCli => "gemini_cli",
    };
    let target_system = match session.target_system {
        InstallTargetSystem::Macos => "macos",
        InstallTargetSystem::Linux => "linux",
        InstallTargetSystem::Windows => "windows",
        InstallTargetSystem::Auto => "auto",
    };

    format!(
        r###"#!/bin/sh
set -eu
TARGET_CLI={target_cli}
TARGET_SYSTEM={target_system}
AETHER_BASE_URL={base_url}
AETHER_API_KEY={api_key}
CLI_LABEL={label}
CLI_BIN={binary}
NPM_PACKAGE={npm_package}

say() {{ printf '%s\n' "[Aether] $1"; }}
fail() {{ printf '%s\n' "[Aether] $1" >&2; exit 1; }}

os="$(uname -s 2>/dev/null || printf unknown)"
case "$os" in
  Darwin) actual_system=macos ;;
  Linux) actual_system=linux ;;
  MINGW*|MSYS*|CYGWIN*) fail "检测到 Windows shell，请在 PowerShell 中使用 Windows 命令：irm <url>.ps1 | iex" ;;
  *) fail "不支持的系统：$os" ;;
esac

if [ "$TARGET_SYSTEM" = "windows" ]; then
  fail "该 install code 绑定 Windows，请复制 PowerShell 命令执行。"
fi
if [ "$TARGET_SYSTEM" != "auto" ] && [ "$TARGET_SYSTEM" != "$actual_system" ]; then
  fail "所选系统 $TARGET_SYSTEM 与当前系统 $actual_system 不一致，请回到 Aether 重新选择目标系统。"
fi

say "准备安装/复用 $CLI_LABEL"
if ! command -v "$CLI_BIN" >/dev/null 2>&1; then
  command -v npm >/dev/null 2>&1 || fail "未找到 $CLI_BIN，也未找到 npm。请先安装 Node.js/npm 后重试。"
  say "未找到 $CLI_BIN，正在通过 npm 安装 $NPM_PACKAGE"
  npm install -g "$NPM_PACKAGE"
fi

umask 077
mkdir -p "$HOME/.aether"
cat > "$HOME/.aether/client.env" <<EOF
AETHER_BASE_URL=$AETHER_BASE_URL
AETHER_API_KEY=$AETHER_API_KEY
EOF
chmod 600 "$HOME/.aether/client.env" 2>/dev/null || true

case "$TARGET_CLI" in
  claude_code)
    mkdir -p "$HOME/.claude"
    python3 - "$HOME/.claude/settings.json" "$AETHER_BASE_URL" "$AETHER_API_KEY" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text() or '{{}}') if path.exists() else {{}}
env = data.setdefault('env', {{}})
env['ANTHROPIC_BASE_URL'] = sys.argv[2]
env['ANTHROPIC_AUTH_TOKEN'] = sys.argv[3]
path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
PY
    chmod 600 "$HOME/.claude/settings.json" 2>/dev/null || true
    ;;
  codex_cli)
    mkdir -p "$HOME/.codex"
    python3 - "$HOME/.codex/config.toml" "$AETHER_BASE_URL" "$AETHER_API_KEY" <<'PY'
import pathlib, re, sys

path = pathlib.Path(sys.argv[1])
base_url = sys.argv[2].rstrip('/') + '/v1'
api_key = sys.argv[3]
text = path.read_text() if path.exists() else ''
lines = text.splitlines()

def quote_toml(value: str) -> str:
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'

result = []
in_aether = False
top_model_provider_set = False
seen_section = False
for line in lines:
    stripped = line.strip()
    if re.match(r'^\[.*\]$', stripped):
        seen_section = True
        in_aether = stripped == '[model_providers.aether]'
        if in_aether:
            continue
    if in_aether:
        continue
    if not seen_section and re.match(r'^model_provider\s*=', stripped):
        if not top_model_provider_set:
            result.append('model_provider = "aether"')
            top_model_provider_set = True
        continue
    result.append(line)

if not top_model_provider_set:
    insert_at = next((idx for idx, line in enumerate(result) if line.strip().startswith('[')), len(result))
    while insert_at > 0 and result[insert_at - 1].strip() == '':
        insert_at -= 1
    result[insert_at:insert_at] = ['model_provider = "aether"', '']

while result and result[-1].strip() == '':
    result.pop()
if result:
    result.append('')
result.extend([
    '# Managed by Aether',
    '[model_providers.aether]',
    'name = "Aether"',
    f'base_url = {{quote_toml(base_url)}}',
    'wire_api = "responses"',
    'requires_openai_auth = false',
    f'experimental_bearer_token = {{quote_toml(api_key)}}',
])
path.write_text('\n'.join(result) + '\n')
PY
    chmod 600 "$HOME/.codex/config.toml" 2>/dev/null || true
    ;;
  gemini_cli)
    mkdir -p "$HOME/.gemini"
    cat > "$HOME/.gemini/.env" <<EOF
GEMINI_API_KEY=$AETHER_API_KEY
GOOGLE_API_KEY=$AETHER_API_KEY
GOOGLE_GEMINI_BASE_URL=$AETHER_BASE_URL
AETHER_BASE_URL=$AETHER_BASE_URL
EOF
    python3 - "$HOME/.gemini/settings.json" "$AETHER_BASE_URL" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text() or '{{}}') if path.exists() else {{}}
data.setdefault('aether', {{}})['baseUrl'] = sys.argv[2]
path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
PY
    chmod 600 "$HOME/.gemini/.env" "$HOME/.gemini/settings.json" 2>/dev/null || true
    ;;
esac

say "$CLI_LABEL 已配置到 Aether。执行 $CLI_BIN --version 验证安装。"
"###,
        target_cli = target_cli,
        target_system = target_system,
        base_url = shell_single_quote(&session.base_url),
        api_key = shell_single_quote(api_key),
        label = shell_single_quote(cli_label(session.target_cli)),
        binary = shell_single_quote(cli_binary(session.target_cli)),
        npm_package = shell_single_quote(npm_package(session.target_cli)),
    )
}

fn build_powershell_script(session: &StoredInstallSession, api_key: &str) -> String {
    let target_cli = match session.target_cli {
        InstallTargetCli::ClaudeCode => "claude_code",
        InstallTargetCli::CodexCli => "codex_cli",
        InstallTargetCli::GeminiCli => "gemini_cli",
    };
    format!(
        r###"$ErrorActionPreference = 'Stop'
$TargetCli = {target_cli}
$TargetSystem = {target_system}
$AetherBaseUrl = {base_url}
$AetherApiKey = {api_key}
$CliLabel = {label}
$CliBin = {binary}
$NpmPackage = {npm_package}

function Say($Message) {{ Write-Host "[Aether] $Message" }}
function Fail($Message) {{ Write-Error "[Aether] $Message"; exit 1 }}

if ($TargetSystem -ne 'auto' -and $TargetSystem -ne 'windows') {{ Fail "该 install code 绑定 $TargetSystem，请复制 macOS/Linux 命令执行。" }}

Say "准备安装/复用 $CliLabel"
if (-not (Get-Command $CliBin -ErrorAction SilentlyContinue)) {{
  if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {{ Fail "未找到 $CliBin，也未找到 npm。请先安装 Node.js/npm 后重试。" }}
  Say "未找到 $CliBin，正在通过 npm 安装 $NpmPackage"
  npm install -g $NpmPackage
}}

$HomeDir = [Environment]::GetFolderPath('UserProfile')
$AetherDir = Join-Path $HomeDir '.aether'
New-Item -ItemType Directory -Force -Path $AetherDir | Out-Null
Set-Content -Path (Join-Path $AetherDir 'client.env') -Value "AETHER_BASE_URL=$AetherBaseUrl`nAETHER_API_KEY=$AetherApiKey`n" -Encoding UTF8

if ($TargetCli -eq 'claude_code') {{
  $Dir = Join-Path $HomeDir '.claude'; New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  $Path = Join-Path $Dir 'settings.json'
  $Data = if (Test-Path $Path) {{ Get-Content $Path -Raw | ConvertFrom-Json -AsHashtable }} else {{ @{{}} }}
  if (-not $Data.ContainsKey('env')) {{ $Data.env = @{{}} }}
  $Data.env.ANTHROPIC_BASE_URL = $AetherBaseUrl
  $Data.env.ANTHROPIC_AUTH_TOKEN = $AetherApiKey
  $Data | ConvertTo-Json -Depth 8 | Set-Content $Path -Encoding UTF8
}} elseif ($TargetCli -eq 'codex_cli') {{
  $Dir = Join-Path $HomeDir '.codex'; New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  $Path = Join-Path $Dir 'config.toml'
  $Text = if (Test-Path $Path) {{ Get-Content $Path -Raw }} else {{ '' }}
  $Lines = if ($Text.Length -gt 0) {{ $Text -split "`r?`n" }} else {{ @() }}
  $Result = New-Object System.Collections.Generic.List[string]
  $InAether = $false
  $TopModelProviderSet = $false
  $SeenSection = $false
  foreach ($Line in $Lines) {{
    $Stripped = $Line.Trim()
    if ($Stripped -match '^\[.*\]$') {{
      $SeenSection = $true
      $InAether = $Stripped -eq '[model_providers.aether]'
      if ($InAether) {{ continue }}
    }}
    if ($InAether) {{ continue }}
    if (-not $SeenSection -and $Stripped -match '^model_provider\s*=') {{
      if (-not $TopModelProviderSet) {{
        $Result.Add('model_provider = "aether"')
        $TopModelProviderSet = $true
      }}
      continue
    }}
    $Result.Add($Line)
  }}
  if (-not $TopModelProviderSet) {{
    $InsertAt = $Result.Count
    for ($Index = 0; $Index -lt $Result.Count; $Index++) {{
      if ($Result[$Index].Trim().StartsWith('[')) {{ $InsertAt = $Index; break }}
    }}
    while ($InsertAt -gt 0 -and $Result[$InsertAt - 1].Trim() -eq '') {{ $InsertAt-- }}
    $Result.Insert($InsertAt, '')
    $Result.Insert($InsertAt, 'model_provider = "aether"')
  }}
  while ($Result.Count -gt 0 -and $Result[$Result.Count - 1].Trim() -eq '') {{ $Result.RemoveAt($Result.Count - 1) }}
  if ($Result.Count -gt 0) {{ $Result.Add('') }}
  $EscapedBaseUrl = ($AetherBaseUrl.TrimEnd('/') + '/v1').Replace('\', '\\').Replace('"', '\"')
  $EscapedApiKey = $AetherApiKey.Replace('\', '\\').Replace('"', '\"')
  $Result.Add('# Managed by Aether')
  $Result.Add('[model_providers.aether]')
  $Result.Add('name = "Aether"')
  $Result.Add("base_url = `"$EscapedBaseUrl`"")
  $Result.Add('wire_api = "responses"')
  $Result.Add('requires_openai_auth = false')
  $Result.Add("experimental_bearer_token = `"$EscapedApiKey`"")
  Set-Content -Path $Path -Value (($Result -join "`n") + "`n") -Encoding UTF8
}} elseif ($TargetCli -eq 'gemini_cli') {{
  $Dir = Join-Path $HomeDir '.gemini'; New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  Set-Content (Join-Path $Dir '.env') -Value "GEMINI_API_KEY=$AetherApiKey`nGOOGLE_API_KEY=$AetherApiKey`nGOOGLE_GEMINI_BASE_URL=$AetherBaseUrl`nAETHER_BASE_URL=$AetherBaseUrl`n" -Encoding UTF8
  $Path = Join-Path $Dir 'settings.json'
  $Data = if (Test-Path $Path) {{ Get-Content $Path -Raw | ConvertFrom-Json -AsHashtable }} else {{ @{{}} }}
  $Data.aether = @{{ baseUrl = $AetherBaseUrl }}
  $Data | ConvertTo-Json -Depth 8 | Set-Content $Path -Encoding UTF8
}}

Say "$CliLabel 已配置到 Aether。执行 $CliBin --version 验证安装。"
"###,
        target_cli = powershell_single_quote(target_cli),
        target_system = powershell_single_quote(match session.target_system {
            InstallTargetSystem::Macos => "macos",
            InstallTargetSystem::Linux => "linux",
            InstallTargetSystem::Windows => "windows",
            InstallTargetSystem::Auto => "auto",
        }),
        base_url = powershell_single_quote(&session.base_url),
        api_key = powershell_single_quote(api_key),
        label = powershell_single_quote(cli_label(session.target_cli)),
        binary = powershell_single_quote(cli_binary(session.target_cli)),
        npm_package = powershell_single_quote(npm_package(session.target_cli)),
    )
}

pub(super) async fn handle_users_me_api_key_install_session_create(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api_key_id) =
        users_me_api_key_install_session_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload = match serde_json::from_slice::<CreateApiKeyInstallSessionRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };

    let records = match state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&auth.user.id))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user api key lookup failed: {err:?}"),
                false,
            )
        }
    };
    let mut matching_records = records
        .into_iter()
        .filter(|record| !record.is_standalone && record.api_key_id == api_key_id);
    let Some(record) = matching_records.next() else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
    if matching_records.next().is_some() {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "API密钥记录不唯一，拒绝创建安装会话",
            false,
        );
    }
    build_api_key_install_session_response(
        state,
        request_context,
        headers,
        remote_addr,
        &record,
        payload,
    )
    .await
}

pub(crate) async fn build_api_key_install_session_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    payload: CreateApiKeyInstallSessionRequest,
) -> Response<Body> {
    let now_unix_secs = unix_secs_now();
    match api_key_record_has_current_snapshot(state, record, now_unix_secs).await {
        Ok(true) => {}
        Ok(false) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "该密钥已停用、锁定、过期或所有者不可用",
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                format!("install session key snapshot validation failed: {err:?}"),
                false,
            )
        }
    }
    let Some(ciphertext) = record
        .key_encrypted
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "该密钥没有存储完整密钥信息",
            false,
        );
    };
    let current_api_key = match decrypt_or_migrate_auth_api_key_secret(state, record).await {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "解密或校验密钥失败",
                false,
            )
        }
    };
    if sha256_hex(&current_api_key) != record.key_hash {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "该密钥完整性校验失败",
            false,
        );
    }

    let code = generate_install_code();
    let expires_at_unix_secs = now_unix_secs.saturating_add(INSTALL_SESSION_TTL_SECS);
    let session = StoredInstallSession {
        install_code: code.clone(),
        api_key_id: record.api_key_id.clone(),
        api_key_owner_user_id: record.user_id.clone(),
        api_key_is_standalone: record.is_standalone,
        api_key_hash: record.key_hash.clone(),
        base_url: base_url_from_request(headers, request_context, remote_addr),
        target_cli: payload.target_cli,
        target_system: payload.target_system,
        expires_at_unix_secs,
    };
    let serialized = match serde_json::to_string(&session) {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("install session serialize failed: {err:?}"),
                false,
            )
        }
    };
    let Some(sealed) = seal_install_session(state, &serialized) else {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "安装会话加密不可用",
            false,
        );
    };
    if let Err(err) = state
        .runtime_kv_setex(
            &install_session_runtime_key(&code),
            &sealed,
            INSTALL_SESSION_TTL_SECS,
        )
        .await
    {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("install session create failed: {err:?}"),
            false,
        );
    }

    let base_url = session.base_url.trim_end_matches('/');
    let unix_url = shell_single_quote(&format!("{base_url}/install/{code}"));
    let powershell_url = powershell_single_quote(&format!("{base_url}/install/{code}.ps1"));
    mark_sensitive_response_no_store(
        Json(json!({
            "install_code": code,
            "expires_at_unix_secs": expires_at_unix_secs,
            "expires_in_seconds": INSTALL_SESSION_TTL_SECS,
            "target_cli": session.target_cli,
            "target_cli_label": cli_label(session.target_cli),
            "target_system": session.target_system,
            "target_system_label": system_label(session.target_system),
            "unix_command": format!("curl -fsSL {unix_url} | sh"),
            "powershell_command": format!("irm {powershell_url} | iex"),
        }))
        .into_response(),
    )
}

pub(crate) async fn build_proxy_node_install_session_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
    node_name: String,
    management_token_record: &aether_data::repository::management_tokens::StoredManagementToken,
    management_token: String,
) -> Response<Body> {
    let current_user = match state
        .find_user_auth_by_id(&management_token_record.user_id)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "隧道安装令牌所有者不可用",
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                format!("tunnel install administrator snapshot lookup failed: {err:?}"),
                false,
            )
        }
    };
    if current_user.id != management_token_record.user_id
        || !current_user.is_active
        || current_user.is_deleted
        || !current_user.role.eq_ignore_ascii_case("admin")
        || current_user.security_version < 0
    {
        if current_user.security_version >= 0 {
            discard_unused_tunnel_install_management_token(
                state,
                management_token_record,
                &management_token,
                current_user.security_version,
            )
            .await;
        }
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "隧道安装令牌所有者不再具有有效管理员身份",
            false,
        );
    }
    if management_token_record.is_active
        || management_token_record.permissions != Some(json!(["admin:proxy_nodes:write"]))
    {
        discard_unused_tunnel_install_management_token(
            state,
            management_token_record,
            &management_token,
            current_user.security_version,
        )
        .await;
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "隧道安装令牌初始化状态无效",
            false,
        );
    }
    let code = generate_install_code();
    let expires_at_unix_secs = unix_secs_now().saturating_add(INSTALL_SESSION_TTL_SECS);
    let session = StoredTunnelInstallSession {
        install_code: code.clone(),
        aether_url: base_url_from_request(headers, request_context, remote_addr),
        management_token_snapshot: management_token_record.clone(),
        management_token_user_security_version: current_user.security_version,
        management_token,
        node_name,
        tunnel_security: "non_tls_required".to_string(),
        tunnel_encryption_key: generate_tunnel_encryption_key(),
        expires_at_unix_secs,
    };
    let serialized = match serde_json::to_string(&session) {
        Ok(value) => value,
        Err(err) => {
            discard_unused_tunnel_install_management_token(
                state,
                management_token_record,
                &session.management_token,
                session.management_token_user_security_version,
            )
            .await;
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("tunnel install session serialize failed: {err:?}"),
                false,
            );
        }
    };
    let Some(sealed) = seal_install_session(state, &serialized) else {
        discard_unused_tunnel_install_management_token(
            state,
            management_token_record,
            &session.management_token,
            session.management_token_user_security_version,
        )
        .await;
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "隧道安装会话加密不可用",
            false,
        );
    };
    if let Err(err) = state
        .runtime_kv_setex(
            &tunnel_install_session_runtime_key(&code),
            &sealed,
            INSTALL_SESSION_TTL_SECS,
        )
        .await
    {
        discard_unused_tunnel_install_management_token(
            state,
            management_token_record,
            &session.management_token,
            session.management_token_user_security_version,
        )
        .await;
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("tunnel install session create failed: {err:?}"),
            false,
        );
    }

    let base_url = session.aether_url.trim_end_matches('/');
    let unix_url = shell_single_quote(&format!("{base_url}/install-tunnel/{code}"));
    let powershell_url = powershell_single_quote(&format!("{base_url}/install-tunnel/{code}.ps1"));
    mark_sensitive_response_no_store(
        Json(json!({
            "install_code": code,
            "expires_at_unix_secs": expires_at_unix_secs,
            "expires_in_seconds": INSTALL_SESSION_TTL_SECS,
            "node_name": session.node_name,
            "aether_url": session.aether_url,
            "unix_command": format!("curl -fsSL {unix_url} | sh"),
            "powershell_command": format!("irm {powershell_url} | iex"),
        }))
        .into_response(),
    )
}

pub(super) async fn maybe_build_local_install_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("install") {
        return None;
    }
    if request_context.request_path.starts_with("/install-tunnel/") {
        return Some(maybe_build_local_tunnel_install_response(state, request_context).await);
    }
    let Some((code, wants_powershell)) = install_code_from_path(&request_context.request_path)
    else {
        return Some(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "install code 不存在或已失效",
            false,
        ));
    };
    let raw = match state
        .runtime_kv_getdel(&install_session_runtime_key(&code))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return Some(build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "install code 不存在、已过期或已使用",
                false,
            ))
        }
        Err(err) => {
            return Some(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("install session lookup failed: {err:?}"),
                false,
            ))
        }
    };
    let Some(raw) = open_install_session(state, &raw) else {
        return Some(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "install code 数据无效",
            false,
        ));
    };
    let session = match serde_json::from_str::<StoredInstallSession>(&raw) {
        Ok(value) => value,
        Err(_) => {
            return Some(build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "install code 数据无效",
                false,
            ))
        }
    };
    if session.install_code != code {
        return Some(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "install code 数据绑定无效",
            false,
        ));
    }
    if session.expires_at_unix_secs <= unix_secs_now() {
        return Some(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "install code 已过期",
            false,
        ));
    }
    let api_key =
        match resolve_current_install_session_api_key(state, &session, unix_secs_now()).await {
            Ok(Some(api_key)) => api_key,
            Ok(None) => {
                return Some(build_auth_error_response(
                    http::StatusCode::NOT_FOUND,
                    "install code 已失效或关联密钥不可用",
                    false,
                ))
            }
            Err(err) => {
                return Some(build_auth_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    format!("install session key validation failed: {err:?}"),
                    false,
                ))
            }
        };
    let body = if wants_powershell {
        build_powershell_script(&session, &api_key)
    } else {
        build_unix_script(&session, &api_key)
    };
    let content_type = if wants_powershell {
        "text/plain; charset=utf-8"
    } else {
        "text/x-shellscript; charset=utf-8"
    };
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static(content_type),
    );
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        http::header::PRAGMA,
        http::HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        http::header::HeaderName::from_static("x-content-type-options"),
        http::HeaderValue::from_static("nosniff"),
    );
    Some(response)
}

async fn maybe_build_local_tunnel_install_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Response<Body> {
    let Some((code, wants_powershell)) =
        tunnel_install_code_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "tunnel install code 不存在或已失效",
            false,
        );
    };
    let raw = match state
        .runtime_kv_getdel(&tunnel_install_session_runtime_key(&code))
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "tunnel install code 不存在、已过期或已使用",
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("tunnel install session lookup failed: {err:?}"),
                false,
            )
        }
    };
    let Some(raw) = open_install_session(state, &raw) else {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "tunnel install code 数据无效",
            false,
        );
    };
    let session = match serde_json::from_str::<StoredTunnelInstallSession>(&raw) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "tunnel install code 数据无效",
                false,
            )
        }
    };
    if session.install_code != code {
        discard_tunnel_install_session_token(state, &session).await;
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "tunnel install code 数据绑定无效",
            false,
        );
    }
    if session.expires_at_unix_secs <= unix_secs_now() {
        discard_tunnel_install_session_token(state, &session).await;
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "tunnel install code 已过期",
            false,
        );
    }
    match activate_tunnel_install_management_token(state, &session, unix_secs_now()).await {
        Ok(true) => {}
        Ok(false) => {
            discard_tunnel_install_session_token(state, &session).await;
            return build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "tunnel install code 已失效或关联令牌不可用",
                false,
            );
        }
        Err(err) => {
            discard_tunnel_install_session_token(state, &session).await;
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                format!("tunnel install token activation failed: {err:?}"),
                false,
            );
        }
    }
    let body = if wants_powershell {
        build_tunnel_powershell_script(&session)
    } else {
        build_tunnel_unix_script(&session)
    };
    let content_type = if wants_powershell {
        "text/plain; charset=utf-8"
    } else {
        "text/x-shellscript; charset=utf-8"
    };
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static(content_type),
    );
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        http::header::PRAGMA,
        http::HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        http::header::HeaderName::from_static("x-content-type-options"),
        http::HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::auth::{
        AuthApiKeyWriteRepository, InMemoryAuthApiKeySnapshotRepository,
        StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
    };

    fn test_session(target_cli: InstallTargetCli) -> StoredInstallSession {
        StoredInstallSession {
            install_code: "0123456789abcdef01234567".to_string(),
            api_key_id: "key-1".to_string(),
            api_key_owner_user_id: "user-1".to_string(),
            api_key_is_standalone: false,
            api_key_hash: "hash-key-1".to_string(),
            base_url: "http://localhost:8084".to_string(),
            target_cli,
            target_system: InstallTargetSystem::Linux,
            expires_at_unix_secs: u64::MAX,
        }
    }

    fn test_tunnel_session() -> StoredTunnelInstallSession {
        StoredTunnelInstallSession {
            install_code: "0123456789abcdef01234567".to_string(),
            aether_url: "https://aether.example".to_string(),
            management_token_snapshot:
                aether_data::repository::management_tokens::StoredManagementToken::new(
                    "token-1".to_string(),
                    "admin-1".to_string(),
                    "tunnel install token".to_string(),
                )
                .expect("management token should build")
                .with_display_fields(None, Some("ae-test".to_string()), None)
                .with_permissions(Some(json!(["admin:proxy_nodes:write"])))
                .with_runtime_fields(None, None, None, 0, false)
                .with_timestamps(Some(1_700_000_000), Some(1_700_000_000)),
            management_token_user_security_version: 3,
            management_token: "ae-test-token".to_string(),
            node_name: "jp-proxy-01".to_string(),
            tunnel_security: "non_tls_required".to_string(),
            tunnel_encryption_key: "base64-32-bytes".to_string(),
            expires_at_unix_secs: u64::MAX,
        }
    }

    fn test_user_api_key_snapshot() -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "user".to_string(),
            Some("user@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            "key-1".to_string(),
            Some("Key 1".to_string()),
            true,
            false,
            false,
            None,
            None,
            Some(4_102_444_800),
            None,
            None,
            None,
        )
        .expect("snapshot should build")
    }

    fn test_user_api_key_export(api_key: &str) -> StoredAuthApiKeyExportRecord {
        StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            sha256_hex(api_key),
            Some(
                encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, api_key)
                    .expect("API key should encrypt"),
            ),
            Some("Key 1".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            Some(4_102_444_800),
            false,
            0,
            0,
            0.0,
            false,
        )
        .expect("export record should build")
    }

    #[test]
    fn install_session_runtime_envelope_encrypts_credentials_and_round_trips() {
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let plaintext = serde_json::to_string(&test_session(InstallTargetCli::CodexCli))
            .expect("session should serialize");
        assert!(!plaintext.contains("sk-test"));
        assert!(!plaintext.contains("\"api_key\":"));

        let sealed = seal_install_session(&state, &plaintext)
            .expect("configured state should seal install session");
        assert!(sealed.starts_with(INSTALL_SESSION_ENVELOPE_PREFIX));
        assert_eq!(
            open_install_session(&state, &sealed).as_deref(),
            Some(plaintext.as_str())
        );
    }

    #[test]
    fn install_session_runtime_keys_do_not_retain_bearer_codes() {
        let code = "0123456789abcdef01234567";
        for runtime_key in [
            install_session_runtime_key(code),
            tunnel_install_session_runtime_key(code),
        ] {
            assert!(runtime_key.contains("sha256:"));
            assert!(!runtime_key.contains(code));
        }
    }

    #[test]
    fn tunnel_install_mutation_binds_full_token_and_admin_security_snapshots() {
        let session = test_tunnel_session();
        let mutation = tunnel_install_token_mutation(&session, 1_700_000_100);

        assert_eq!(mutation.expected_token, session.management_token_snapshot);
        assert_eq!(
            mutation.expected_user_security_version,
            session.management_token_user_security_version
        );
        assert_eq!(mutation.token_hash, sha256_hex(&session.management_token));
        assert_eq!(mutation.now_unix_secs, 1_700_000_100);
    }

    #[tokio::test]
    async fn install_session_strong_revalidation_rejects_new_lock_with_stale_cache() {
        let api_key = "sk-test-lock-revalidation";
        let repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed([(
                Some(sha256_hex(api_key)),
                test_user_api_key_snapshot(),
            )])
            .with_export_records([test_user_api_key_export(api_key)]),
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_cached_auth_api_key_repository_for_tests(
                    Arc::clone(&repository),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let mut session = test_session(InstallTargetCli::CodexCli);
        session.api_key_hash = sha256_hex(api_key);

        let now_unix_secs = unix_secs_now();
        let cached = state
            .data
            .read_auth_api_key_snapshot("user-1", "key-1", now_unix_secs)
            .await
            .expect("initial snapshot should load")
            .expect("initial snapshot should exist");
        assert!(!cached.api_key_is_locked);
        assert_eq!(
            resolve_current_install_session_api_key(&state, &session, now_unix_secs)
                .await
                .expect("initial strong validation should complete")
                .as_deref(),
            Some(api_key),
        );

        repository
            .set_user_api_key_locked("user-1", "key-1", true)
            .await
            .expect("lock update should succeed");
        let stale = state
            .data
            .read_auth_api_key_snapshot("user-1", "key-1", now_unix_secs)
            .await
            .expect("cached snapshot should load")
            .expect("cached snapshot should exist");
        assert!(
            !stale.api_key_is_locked,
            "test must retain a stale cache entry"
        );

        assert_eq!(
            resolve_current_install_session_api_key(&state, &session, now_unix_secs,)
                .await
                .expect("strong validation should complete"),
            None,
        );
    }

    #[test]
    fn install_session_reader_rejects_legacy_plaintext() {
        let state = AppState::new().expect("state should build");
        let legacy = r#"{"api_key":"legacy-secret"}"#;
        assert!(open_install_session(&state, legacy).is_none());
    }

    #[test]
    fn tunnel_install_path_accepts_shell_and_powershell_codes() {
        let code = "0123456789abcdef01234567";
        assert_eq!(
            tunnel_install_code_from_path(&format!("/install-tunnel/{code}")),
            Some((code.to_string(), false))
        );
        assert_eq!(
            tunnel_install_code_from_path(&format!("/install-tunnel/{code}.ps1")),
            Some((code.to_string(), true))
        );
        assert_eq!(tunnel_install_code_from_path("/install-tunnel/a/b"), None);
        assert_eq!(
            tunnel_install_code_from_path("/install-tunnel/abc123"),
            None
        );
        assert_eq!(
            tunnel_install_code_from_path("/install-tunnel/0123456789ABCDEF01234567"),
            None
        );
    }

    #[test]
    fn public_base_url_rejects_credentials_queries_and_shell_metacharacters() {
        assert_eq!(
            normalize_public_base_url(" https://aether.example/base/ "),
            Some("https://aether.example/base".to_string())
        );
        assert_eq!(
            normalize_public_base_url("http://127.0.0.1:8084/"),
            Some("http://127.0.0.1:8084".to_string())
        );
        for value in [
            "http://aether.example",
            "http://10.0.0.8:8084",
            "https://user:secret@aether.example",
            "https://aether.example?token=secret",
            "https://aether.example/#fragment",
            "https://aether.example';touch /tmp/pwned;'",
            "javascript:alert(1)",
        ] {
            assert_eq!(
                normalize_public_base_url(value),
                None,
                "unsafe URL: {value}"
            );
        }
    }

    #[test]
    fn untrusted_peer_cannot_poison_generated_install_base_url() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-forwarded-host", "attacker.example".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());

        assert_eq!(
            base_url_from_request_metadata(&headers, Some("also-attacker.example"), false),
            "http://localhost"
        );
        assert_eq!(
            base_url_from_request_metadata(&headers, Some("gateway.example"), true),
            "https://attacker.example"
        );
    }

    #[test]
    fn trusted_proxy_uses_rightmost_forwarded_host_and_proto() {
        let mut headers = http::HeaderMap::new();
        headers.append(
            "x-forwarded-host",
            "client-injected.example, stale-proxy.example"
                .parse()
                .unwrap(),
        );
        headers.append("x-forwarded-host", "gateway.example".parse().unwrap());
        headers.append("x-forwarded-proto", "http, http".parse().unwrap());
        headers.append("x-forwarded-proto", "https".parse().unwrap());

        assert_eq!(
            base_url_from_request_metadata(&headers, Some("ignored.example"), true),
            "https://gateway.example"
        );
    }

    #[test]
    fn tunnel_unix_script_exports_session_values_and_reuses_tunnel_installer() {
        let script = build_tunnel_unix_script(&test_tunnel_session());

        assert!(script.contains("export AETHER_TUNNEL_AETHER_URL='https://aether.example'"));
        assert!(script.contains("export AETHER_TUNNEL_MANAGEMENT_TOKEN='ae-test-token'"));
        assert!(script.contains("export AETHER_TUNNEL_NODE_NAME='jp-proxy-01'"));
        assert!(script.contains("export AETHER_TUNNEL_SECURITY='non_tls_required'"));
        assert!(script.contains("export AETHER_TUNNEL_ENCRYPTION_KEY='base64-32-bytes'"));
        assert!(script.contains(
            "https://raw.githubusercontent.com/fawney19/Aether/refs/heads/main/apps/aether-tunnel/install.sh"
        ));
        assert!(!script.contains("aether-rust-pioneer"));
        assert!(!script.contains("[[servers]]"));
    }

    #[test]
    fn tunnel_powershell_script_exports_session_values_and_reuses_tunnel_installer() {
        let script = build_tunnel_powershell_script(&test_tunnel_session());

        assert!(script.contains("$env:AETHER_TUNNEL_AETHER_URL = 'https://aether.example'"));
        assert!(script.contains("$env:AETHER_TUNNEL_MANAGEMENT_TOKEN = 'ae-test-token'"));
        assert!(script.contains("$env:AETHER_TUNNEL_NODE_NAME = 'jp-proxy-01'"));
        assert!(script.contains("$env:AETHER_TUNNEL_SECURITY = 'non_tls_required'"));
        assert!(script.contains("$env:AETHER_TUNNEL_ENCRYPTION_KEY = 'base64-32-bytes'"));
        assert!(script.contains(
            "https://raw.githubusercontent.com/fawney19/Aether/main/apps/aether-tunnel/install.ps1"
        ));
        assert!(!script.contains("aether-rust-pioneer"));
        assert!(!script.contains("[[servers]]"));
    }

    #[test]
    fn codex_unix_script_preserves_config_and_uses_responses_bearer_token() {
        let script = build_unix_script(&test_session(InstallTargetCli::CodexCli), "sk-test");

        assert!(script.contains("path.read_text() if path.exists() else ''"));
        assert!(script.contains("stripped == '[model_providers.aether]'"));
        assert!(script.contains("model_provider = \"aether\""));
        assert!(script.contains("wire_api = \"responses\""));
        assert!(script.contains("requires_openai_auth = false"));
        assert!(script.contains("experimental_bearer_token ="));
        assert!(!script.contains("wire_api = \"chat\""));
        assert!(!script.contains("cat > \"$HOME/.codex/config.toml\""));
        assert!(!script.contains("auth.json"));
    }

    #[test]
    fn codex_powershell_script_preserves_config_and_uses_responses_bearer_token() {
        let script = build_powershell_script(&test_session(InstallTargetCli::CodexCli), "sk-test");

        assert!(script.contains("Get-Content $Path -Raw"));
        assert!(script.contains("$Stripped -eq '[model_providers.aether]'"));
        assert!(script.contains("model_provider = \"aether\""));
        assert!(script.contains("wire_api = \"responses\""));
        assert!(script.contains("requires_openai_auth = false"));
        assert!(script.contains("experimental_bearer_token ="));
        assert!(!script.contains("wire_api = \"chat\""));
        assert!(!script.contains("auth.json"));
    }
}
