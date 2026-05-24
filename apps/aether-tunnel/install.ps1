$ErrorActionPreference = 'Stop'

$Repo = if ($env:AETHER_TUNNEL_RELEASE_REPO) { $env:AETHER_TUNNEL_RELEASE_REPO } else { 'fawney19/Aether' }
$ReleaseTag = $env:AETHER_TUNNEL_RELEASE_TAG
$InstallDir = $env:AETHER_TUNNEL_INSTALL_DIR
$ConfigPath = $env:AETHER_TUNNEL_CONFIG

function Say([string]$Message) { Write-Host "[Aether Tunnel] $Message" }
function Fail([string]$Message) { throw "[Aether Tunnel] $Message" }

function Prompt-IfEmpty([string]$Name, [string]$Value, [string]$Prompt) {
  if (-not [string]::IsNullOrWhiteSpace($Value)) { return $Value }
  $Read = Read-Host $Prompt
  if ([string]::IsNullOrWhiteSpace($Read)) { Fail "$Name cannot be empty" }
  return $Read
}

function ConvertTo-TomlQuotedString([string]$Value) {
  return ($Value | ConvertTo-Json -Compress)
}

function Resolve-LatestTunnelTag {
  if (-not [string]::IsNullOrWhiteSpace($ReleaseTag)) { return $ReleaseTag }
  $Uri = "https://api.github.com/repos/$Repo/releases?per_page=100"
  $Releases = Invoke-RestMethod -Uri $Uri -Headers @{ 'User-Agent' = 'aether-tunnel-installer' }
  $TunnelReleases = @($Releases | Where-Object { -not $_.draft -and $_.tag_name -like 'tunnel-v*' } | Sort-Object published_at -Descending)
  if ($TunnelReleases.Count -eq 0) { Fail "No tunnel-v* release found in $Repo" }
  return $TunnelReleases[0].tag_name
}

function Test-IsAdministrator {
  $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $Principal = [Security.Principal.WindowsPrincipal]::new($Identity)
  return $Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Initialize-Paths {
  if ([string]::IsNullOrWhiteSpace($script:InstallDir)) {
    if (Test-IsAdministrator) {
      $script:InstallDir = Join-Path $env:ProgramFiles 'AetherTunnel'
    } else {
      $script:InstallDir = Join-Path $env:LOCALAPPDATA 'AetherTunnel'
    }
  }
  if ([string]::IsNullOrWhiteSpace($script:ConfigPath)) {
    if (Test-IsAdministrator) {
      $script:ConfigPath = Join-Path $env:ProgramData 'AetherTunnel\aether-tunnel.toml'
    } else {
      $script:ConfigPath = Join-Path $env:APPDATA 'AetherTunnel\aether-tunnel.toml'
    }
  }
}

function Install-AetherTunnelBinary([string]$Tag, [string]$TempDir) {
  if (-not [Environment]::Is64BitOperatingSystem) { Fail 'Windows release currently supports amd64 only' }
  $Asset = 'aether-tunnel-windows-amd64.zip'
  $Base = "https://github.com/$Repo/releases/download/$Tag"
  $Archive = Join-Path $TempDir $Asset
  $Sums = Join-Path $TempDir 'SHA256SUMS.txt'

  Say "Downloading $Tag / $Asset"
  Invoke-WebRequest -Uri "$Base/$Asset" -OutFile $Archive
  try { Invoke-WebRequest -Uri "$Base/SHA256SUMS.txt" -OutFile $Sums } catch { $Sums = $null }

  if ($Sums -and (Test-Path $Sums)) {
    $ExpectedLine = Get-Content $Sums | Where-Object { $_ -match "\s$([regex]::Escape($Asset))$" } | Select-Object -First 1
    if ($ExpectedLine) {
      $Expected = ($ExpectedLine -split '\s+')[0]
      $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
      if ($Actual -ne $Expected.ToLowerInvariant()) { Fail "SHA256 verification failed for $Asset" }
    }
  }

  $ExtractDir = Join-Path $TempDir 'extract'
  Expand-Archive -Path $Archive -DestinationPath $ExtractDir -Force
  $Binary = Join-Path $ExtractDir 'aether-tunnel.exe'
  if (-not (Test-Path $Binary)) { Fail 'aether-tunnel.exe not found in release asset' }
  New-Item -ItemType Directory -Force -Path $script:InstallDir | Out-Null
  Copy-Item $Binary (Join-Path $script:InstallDir 'aether-tunnel.exe') -Force
  Say "Installed binary: $(Join-Path $script:InstallDir 'aether-tunnel.exe')"
}

function Test-LegacySingleServerConfig([string]$Path) {
  if (-not (Test-Path $Path)) { return $false }
  foreach ($Line in Get-Content $Path) {
    if ($Line -match '^\s*\[') { return $false }
    if ($Line -match '^\s*(aether_url|management_token)\s*=') { return $true }
  }
  return $false
}

function Test-ServerExists([string]$Path, [string]$QuotedUrl, [string]$QuotedName) {
  if (-not (Test-Path $Path)) { return $false }
  $FoundUrl = $false
  $FoundName = $false
  foreach ($Line in Get-Content $Path) {
    if ($Line -match '^\s*\[\[servers\]\]\s*$') {
      if ($FoundUrl -and $FoundName) { return $true }
      $FoundUrl = $false
      $FoundName = $false
    }
    if ($Line.Trim() -eq "aether_url = $QuotedUrl") { $FoundUrl = $true }
    if ($Line.Trim() -eq "node_name = $QuotedName") { $FoundName = $true }
  }
  return ($FoundUrl -and $FoundName)
}

function Add-ServerConfig([string]$AetherUrl, [string]$ManagementToken, [string]$NodeName, [string]$TunnelSecurity, [string]$TunnelEncryptionKey) {
  $ConfigDir = Split-Path -Parent $script:ConfigPath
  New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null

  if (Test-LegacySingleServerConfig $script:ConfigPath) {
    Fail "Existing config uses removed top-level aether_url/management_token. Run aether-tunnel setup to migrate to [[servers]] first: $script:ConfigPath"
  }

  $QuotedUrl = ConvertTo-TomlQuotedString $AetherUrl
  $QuotedToken = ConvertTo-TomlQuotedString $ManagementToken
  $QuotedName = ConvertTo-TomlQuotedString $NodeName
  $QuotedTunnelEncryptionKey = ConvertTo-TomlQuotedString $TunnelEncryptionKey

  if (Test-ServerExists $script:ConfigPath $QuotedUrl $QuotedName) {
    Say "Same aether_url + node_name already exists, skipping config append: $script:ConfigPath"
    return
  }

  if (Test-Path $script:ConfigPath) {
    Copy-Item $script:ConfigPath "$script:ConfigPath.bak.$(Get-Date -Format yyyyMMddHHmmss)" -Force
  }

  $Prefix = if ((Test-Path $script:ConfigPath) -and ((Get-Item $script:ConfigPath).Length -gt 0)) { "`n" } else { '' }
  $Block = @(
    "$Prefix# Added by Aether Tunnel one-click installer. Existing config is preserved.",
    '[[servers]]',
    "aether_url = $QuotedUrl",
    "management_token = $QuotedToken",
    "node_name = $QuotedName"
  ) -join "`n"
  if ($TunnelSecurity) {
    $QuotedTunnelSecurity = ConvertTo-TomlQuotedString $TunnelSecurity
    $Block += "`ntunnel_security = $QuotedTunnelSecurity"
  }
  if ($TunnelEncryptionKey) {
    $Block += "`ntunnel_encryption_key = $QuotedTunnelEncryptionKey"
  }
  Add-Content -Path $script:ConfigPath -Value ($Block + "`n") -Encoding UTF8
  Say "Appended [[servers]] to: $script:ConfigPath"
}

function Main {
  Initialize-Paths
  $AetherUrl = Prompt-IfEmpty 'AETHER_TUNNEL_AETHER_URL' $env:AETHER_TUNNEL_AETHER_URL 'Aether URL'
  $ManagementToken = Prompt-IfEmpty 'AETHER_TUNNEL_MANAGEMENT_TOKEN' $env:AETHER_TUNNEL_MANAGEMENT_TOKEN 'Management token (ae_xxx)'
  $NodeName = Prompt-IfEmpty 'AETHER_TUNNEL_NODE_NAME' $env:AETHER_TUNNEL_NODE_NAME 'Node name'
  $TunnelSecurity = if ($env:AETHER_TUNNEL_SECURITY) { $env:AETHER_TUNNEL_SECURITY } else { '' }
  $TunnelEncryptionKey = if ($env:AETHER_TUNNEL_ENCRYPTION_KEY) { $env:AETHER_TUNNEL_ENCRYPTION_KEY } else { '' }
  if ($TunnelSecurity -and ($TunnelSecurity -notin @('off', 'non_tls_required'))) {
    Fail 'AETHER_TUNNEL_SECURITY must be off or non_tls_required'
  }
  if (($TunnelSecurity -eq 'non_tls_required') -and -not $TunnelEncryptionKey) {
    Fail 'AETHER_TUNNEL_ENCRYPTION_KEY is required when AETHER_TUNNEL_SECURITY=non_tls_required'
  }

  $TempDir = Join-Path ([IO.Path]::GetTempPath()) ("aether-tunnel-" + [Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
  try {
    $Tag = Resolve-LatestTunnelTag
    Install-AetherTunnelBinary $Tag $TempDir
    Add-ServerConfig $AetherUrl $ManagementToken $NodeName $TunnelSecurity $TunnelEncryptionKey
  } finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
  }

  Say 'Complete. Start or configure the node with:'
  Say "  & '$(Join-Path $script:InstallDir 'aether-tunnel.exe')' setup '$script:ConfigPath'"
}

Main
