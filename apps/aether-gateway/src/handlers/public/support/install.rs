use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    build_auth_error_response, decrypt_catalog_secret_with_fallbacks,
    resolve_authenticated_local_user, AppState, GatewayPublicRequestContext,
};

const INSTALL_SESSION_TTL_SECS: u64 = 15 * 60;
const INSTALL_SESSION_KEY_PREFIX: &str = "install:session:";
const TUNNEL_INSTALL_SESSION_KEY_PREFIX: &str = "tunnel-install:session:";
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
    api_key_id: String,
    api_key_name: String,
    api_key: String,
    base_url: String,
    target_cli: InstallTargetCli,
    target_system: InstallTargetSystem,
    expires_at_unix_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredTunnelInstallSession {
    aether_url: String,
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
    (!code.is_empty()).then(|| (code.to_string(), is_powershell))
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
    (!code.is_empty()).then(|| (code.to_string(), is_powershell))
}

fn install_session_runtime_key(code: &str) -> String {
    format!("{INSTALL_SESSION_KEY_PREFIX}{code}")
}

fn tunnel_install_session_runtime_key(code: &str) -> String {
    format!("{TUNNEL_INSTALL_SESSION_KEY_PREFIX}{code}")
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

fn unix_secs_now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

pub(crate) fn base_url_from_request(
    headers: &http::HeaderMap,
    request_context: &GatewayPublicRequestContext,
) -> String {
    if let Some(value) = std::env::var("AETHER_PUBLIC_BASE_URL")
        .ok()
        .or_else(|| std::env::var("PUBLIC_BASE_URL").ok())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
    {
        return value;
    }

    let host = crate::headers::header_value_str(headers, "x-forwarded-host")
        .or_else(|| request_context.host_header.clone())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| {
            !value.is_empty()
                && !value.contains('/')
                && !value.contains('\\')
                && !value.contains('@')
                && !value.contains(char::is_whitespace)
        })
        .unwrap_or_else(|| "localhost".to_string());
    let proto = crate::headers::header_value_str(headers, "x-forwarded-proto")
        .map(|value| value.trim().trim_end_matches(':').to_ascii_lowercase())
        .filter(|value| value == "http" || value == "https")
        .unwrap_or_else(|| "http".to_string());
    format!("{proto}://{host}")
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

fn build_unix_script(session: &StoredInstallSession) -> String {
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
        api_key = shell_single_quote(&session.api_key),
        label = shell_single_quote(cli_label(session.target_cli)),
        binary = shell_single_quote(cli_binary(session.target_cli)),
        npm_package = shell_single_quote(npm_package(session.target_cli)),
    )
}

fn build_powershell_script(session: &StoredInstallSession) -> String {
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
        api_key = powershell_single_quote(&session.api_key),
        label = powershell_single_quote(cli_label(session.target_cli)),
        binary = powershell_single_quote(cli_binary(session.target_cli)),
        npm_package = powershell_single_quote(npm_package(session.target_cli)),
    )
}

pub(super) async fn handle_users_me_api_key_install_session_create(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
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
    let Some(record) = records
        .into_iter()
        .find(|record| !record.is_standalone && record.api_key_id == api_key_id)
    else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "API密钥不存在", false);
    };
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
    let Some(api_key) = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
    else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "解密密钥失败",
            false,
        );
    };

    build_api_key_install_session_response(
        state,
        request_context,
        headers,
        record.api_key_id.clone(),
        record.name.unwrap_or_else(|| "API Key".to_string()),
        api_key,
        payload,
    )
    .await
}

pub(crate) async fn build_api_key_install_session_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    api_key_id: String,
    api_key_name: String,
    api_key: String,
    payload: CreateApiKeyInstallSessionRequest,
) -> Response<Body> {
    let code = generate_install_code();
    let expires_at_unix_secs = unix_secs_now().saturating_add(INSTALL_SESSION_TTL_SECS);
    let session = StoredInstallSession {
        api_key_id,
        api_key_name,
        api_key,
        base_url: base_url_from_request(headers, request_context),
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
    if let Err(err) = state
        .runtime_kv_setex(
            &install_session_runtime_key(&code),
            &serialized,
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
    Json(json!({
        "install_code": code,
        "expires_at_unix_secs": expires_at_unix_secs,
        "expires_in_seconds": INSTALL_SESSION_TTL_SECS,
        "target_cli": session.target_cli,
        "target_cli_label": cli_label(session.target_cli),
        "target_system": session.target_system,
        "target_system_label": system_label(session.target_system),
        "unix_command": format!("curl -fsSL {base_url}/install/{code} | sh"),
        "powershell_command": format!("irm {base_url}/install/{code}.ps1 | iex"),
    }))
    .into_response()
}

pub(crate) async fn build_proxy_node_install_session_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    node_name: String,
    management_token: String,
) -> Response<Body> {
    let code = generate_install_code();
    let expires_at_unix_secs = unix_secs_now().saturating_add(INSTALL_SESSION_TTL_SECS);
    let session = StoredTunnelInstallSession {
        aether_url: base_url_from_request(headers, request_context),
        management_token,
        node_name,
        tunnel_security: "non_tls_required".to_string(),
        tunnel_encryption_key: generate_tunnel_encryption_key(),
        expires_at_unix_secs,
    };
    let serialized = match serde_json::to_string(&session) {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("tunnel install session serialize failed: {err:?}"),
                false,
            )
        }
    };
    if let Err(err) = state
        .runtime_kv_setex(
            &tunnel_install_session_runtime_key(&code),
            &serialized,
            INSTALL_SESSION_TTL_SECS,
        )
        .await
    {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("tunnel install session create failed: {err:?}"),
            false,
        );
    }

    let base_url = session.aether_url.trim_end_matches('/');
    Json(json!({
        "install_code": code,
        "expires_at_unix_secs": expires_at_unix_secs,
        "expires_in_seconds": INSTALL_SESSION_TTL_SECS,
        "node_name": session.node_name,
        "aether_url": session.aether_url,
        "unix_command": format!("curl -fsSL {base_url}/install-tunnel/{code} | sh"),
        "powershell_command": format!("irm {base_url}/install-tunnel/{code}.ps1 | iex"),
    }))
    .into_response()
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
    if session.expires_at_unix_secs <= unix_secs_now() {
        return Some(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "install code 已过期",
            false,
        ));
    }
    let body = if wants_powershell {
        build_powershell_script(&session)
    } else {
        build_unix_script(&session)
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
    if session.expires_at_unix_secs <= unix_secs_now() {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "tunnel install code 已过期",
            false,
        );
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

    fn test_session(target_cli: InstallTargetCli) -> StoredInstallSession {
        StoredInstallSession {
            api_key_id: "key-1".to_string(),
            api_key_name: "Key 1".to_string(),
            api_key: "sk-test".to_string(),
            base_url: "http://localhost:8084".to_string(),
            target_cli,
            target_system: InstallTargetSystem::Linux,
            expires_at_unix_secs: u64::MAX,
        }
    }

    fn test_tunnel_session() -> StoredTunnelInstallSession {
        StoredTunnelInstallSession {
            aether_url: "https://aether.example".to_string(),
            management_token: "ae-test-token".to_string(),
            node_name: "jp-proxy-01".to_string(),
            tunnel_security: "non_tls_required".to_string(),
            tunnel_encryption_key: "base64-32-bytes".to_string(),
            expires_at_unix_secs: u64::MAX,
        }
    }

    #[test]
    fn tunnel_install_path_accepts_shell_and_powershell_codes() {
        assert_eq!(
            tunnel_install_code_from_path("/install-tunnel/abc123"),
            Some(("abc123".to_string(), false))
        );
        assert_eq!(
            tunnel_install_code_from_path("/install-tunnel/abc123.ps1"),
            Some(("abc123".to_string(), true))
        );
        assert_eq!(tunnel_install_code_from_path("/install-tunnel/a/b"), None);
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
        let script = build_unix_script(&test_session(InstallTargetCli::CodexCli));

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
        let script = build_powershell_script(&test_session(InstallTargetCli::CodexCli));

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
