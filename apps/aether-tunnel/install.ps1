$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http

$Repo = if ($env:AETHER_TUNNEL_RELEASE_REPO) { $env:AETHER_TUNNEL_RELEASE_REPO } else { 'fawney19/Aether' }
$ReleaseTag = $env:AETHER_TUNNEL_RELEASE_TAG
$InstallDir = $env:AETHER_TUNNEL_INSTALL_DIR
$ConfigPath = $env:AETHER_TUNNEL_CONFIG
$TunnelReleaseTagPattern = '^tunnel-v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$'

function Say([string]$Message) { Write-Host "[Aether Tunnel] $Message" }
function Fail([string]$Message) { throw "[Aether Tunnel] $Message" }

function Prompt-IfEmpty([string]$Name, [string]$Value, [string]$Prompt) {
  if (-not [string]::IsNullOrWhiteSpace($Value)) { return $Value }
  $Read = Read-Host $Prompt
  if ([string]::IsNullOrWhiteSpace($Read)) { Fail "$Name cannot be empty" }
  return $Read
}

function Assert-SafeReleaseRepo([string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or
      $Value.Length -gt 200 -or
      $Value -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$') {
    Fail 'Release repository must be a safe GitHub OWNER/REPO identifier'
  }
}

function Assert-SafeTunnelReleaseTag([string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or
      $Value.Length -gt 128 -or
      $Value -cnotmatch $TunnelReleaseTagPattern) {
    Fail 'Release tag must use tunnel-v followed by a valid semantic version'
  }
}

function Assert-TrustedGithubUri([Uri]$Uri) {
  if (-not $Uri.IsAbsoluteUri -or
      $Uri.Scheme -cne 'https' -or
      -not [string]::IsNullOrEmpty($Uri.UserInfo) -or
      -not [string]::IsNullOrEmpty($Uri.Fragment) -or
      $Uri.Port -ne 443) {
    Fail "GitHub downloads must use credential-free HTTPS on port 443: $Uri"
  }
  $HostName = $Uri.IdnHost.ToLowerInvariant()
  $TrustedHost = $HostName -in @('api.github.com', 'github.com', 'objects.githubusercontent.com', 'release-assets.githubusercontent.com') -or
    $HostName.EndsWith('.objects.githubusercontent.com', [StringComparison]::Ordinal) -or
    $HostName.EndsWith('.release-assets.githubusercontent.com', [StringComparison]::Ordinal)
  if (-not $TrustedHost) { Fail "GitHub download redirected to an untrusted host: $HostName" }
}

function Get-TrustedGithubBytes([string]$UriText) {
  $CurrentUri = [Uri]::new($UriText, [UriKind]::Absolute)
  Assert-TrustedGithubUri $CurrentUri
  $Handler = [System.Net.Http.HttpClientHandler]::new()
  $Handler.AllowAutoRedirect = $false
  $Client = [System.Net.Http.HttpClient]::new($Handler, $true)
  $Client.DefaultRequestHeaders.UserAgent.ParseAdd('aether-tunnel-installer')
  try {
    for ($RedirectCount = 0; $RedirectCount -le 10; $RedirectCount++) {
      $Response = $null
      try {
        $Response = $Client.GetAsync(
          $CurrentUri,
          [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead
        ).GetAwaiter().GetResult()
        $StatusCode = [int]$Response.StatusCode
        if ($StatusCode -in @(301, 302, 303, 307, 308)) {
          if ($RedirectCount -eq 10) { Fail 'GitHub download redirected too many times' }
          $Location = $Response.Headers.Location
          if (-not $Location) { Fail 'GitHub redirect is missing the Location header' }
          $CurrentUri = if ($Location.IsAbsoluteUri) {
            $Location
          } else {
            [Uri]::new($CurrentUri, $Location)
          }
          Assert-TrustedGithubUri $CurrentUri
          continue
        }
        if (-not $Response.IsSuccessStatusCode) {
          Fail "GitHub download returned HTTP $StatusCode for $CurrentUri"
        }
        $Bytes = $Response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
        return ,$Bytes
      } finally {
        if ($Response) { $Response.Dispose() }
      }
    }
    Fail 'GitHub download redirected too many times'
  } finally {
    $Client.Dispose()
  }
}

function Read-TrustedGithubJson([string]$Uri) {
  $Bytes = Get-TrustedGithubBytes $Uri
  $Text = [Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
  return ($Text | ConvertFrom-Json)
}

function Save-TrustedGithubFile([string]$Uri, [string]$Path) {
  $Bytes = Get-TrustedGithubBytes $Uri
  $Stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
  try {
    $Stream.Write($Bytes, 0, $Bytes.Length)
    $Stream.Flush($true)
  } finally {
    $Stream.Dispose()
  }
}

function Assert-SafeNodeName([string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or
      $Value.Length -gt 255 -or
      $Value -ne $Value.Trim() -or
      $Value -match '[\x00-\x1F\x7F]') {
    Fail 'Node name must be 1 to 255 characters without surrounding whitespace or control characters'
  }
}

function ConvertTo-TomlQuotedString([string]$Value) {
  return ($Value | ConvertTo-Json -Compress)
}

function Resolve-LatestTunnelTag {
  Assert-SafeReleaseRepo $Repo
  if (-not [string]::IsNullOrWhiteSpace($ReleaseTag)) {
    Assert-SafeTunnelReleaseTag $ReleaseTag
    $RequestedUri = "https://api.github.com/repos/$Repo/releases/tags/$ReleaseTag"
    $RequestedRelease = Read-TrustedGithubJson $RequestedUri
    if ($RequestedRelease.draft -or ([string]$RequestedRelease.tag_name -cne $ReleaseTag)) {
      Fail 'GitHub returned a draft or mismatched tunnel release'
    }
    return $ReleaseTag
  }
  $Uri = "https://api.github.com/repos/$Repo/releases?per_page=100"
  $Releases = Read-TrustedGithubJson $Uri
  $TunnelReleases = @($Releases | Where-Object {
    -not $_.draft -and -not $_.prerelease -and ([string]$_.tag_name -cmatch $TunnelReleaseTagPattern)
  } | Sort-Object published_at -Descending)
  if ($TunnelReleases.Count -eq 0) { Fail "No tunnel-v* release found in $Repo" }
  return $TunnelReleases[0].tag_name
}

function Test-IsAdministrator {
  $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $Principal = [Security.Principal.WindowsPrincipal]::new($Identity)
  return $Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Protect-SensitiveConfigFile([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Fail "Sensitive config file does not exist: $Path"
  }
  Assert-NotReparsePoint $Path 'Sensitive config file'

  $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $CurrentSid = $Identity.User
  if (-not $CurrentSid) { Fail 'Unable to resolve the current Windows account SID' }

  $Acl = [System.Security.AccessControl.FileSecurity]::new()
  $Acl.SetAccessRuleProtection($true, $false)
  $Acl.SetOwner($CurrentSid)

  $AllowedSids = @(
    $CurrentSid.Value,
    'S-1-5-18',
    'S-1-5-32-544'
  ) | Select-Object -Unique
  foreach ($SidValue in $AllowedSids) {
    $Sid = [Security.Principal.SecurityIdentifier]::new($SidValue)
    $Rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
      $Sid,
      [System.Security.AccessControl.FileSystemRights]::FullControl,
      [System.Security.AccessControl.AccessControlType]::Allow
    )
    $Acl.AddAccessRule($Rule) | Out-Null
  }

  Set-Acl -LiteralPath $Path -AclObject $Acl
}

function Protect-SensitiveConfigDirectory([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
    Fail "Sensitive config directory does not exist: $Path"
  }
  Assert-NotReparsePoint $Path 'Sensitive config directory'

  $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $CurrentSid = $Identity.User
  if (-not $CurrentSid) { Fail 'Unable to resolve the current Windows account SID' }

  $Acl = [System.Security.AccessControl.DirectorySecurity]::new()
  $Acl.SetAccessRuleProtection($true, $false)
  $Acl.SetOwner($CurrentSid)
  $AllowedSids = @($CurrentSid.Value, 'S-1-5-18', 'S-1-5-32-544') | Select-Object -Unique
  foreach ($SidValue in $AllowedSids) {
    $Sid = [Security.Principal.SecurityIdentifier]::new($SidValue)
    $Rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
      $Sid,
      [System.Security.AccessControl.FileSystemRights]::FullControl,
      [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
      [System.Security.AccessControl.PropagationFlags]::None,
      [System.Security.AccessControl.AccessControlType]::Allow
    )
    $Acl.AddAccessRule($Rule) | Out-Null
  }
  Set-Acl -LiteralPath $Path -AclObject $Acl
}

function Protect-SensitiveConfigArtifacts([string]$Path) {
  if (Test-Path -LiteralPath $Path -PathType Leaf) {
    Protect-SensitiveConfigFile $Path
  }

  $Directory = Split-Path -Parent $Path
  $Leaf = Split-Path -Leaf $Path
  if (-not (Test-Path -LiteralPath $Directory -PathType Container)) { return }
  foreach ($Backup in Get-ChildItem -LiteralPath $Directory -File) {
    if ($Backup.Name.StartsWith("$Leaf.bak.", [StringComparison]::OrdinalIgnoreCase)) {
      Protect-SensitiveConfigFile $Backup.FullName
    }
  }
}

function Assert-NotReparsePoint([string]$Path, [string]$Description) {
  if (-not (Test-Path -LiteralPath $Path)) { return }
  $Item = Get-Item -LiteralPath $Path -Force
  if (($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    Fail "$Description must not be a reparse point or symbolic link: $Path"
  }
}

function Assert-NoReparsePointAncestors([string]$Path, [string]$Description) {
  $Current = [IO.Path]::GetFullPath($Path)
  while (-not [string]::IsNullOrEmpty($Current)) {
    if (Test-Path -LiteralPath $Current) {
      Assert-NotReparsePoint $Current "$Description ancestor"
    }
    $Parent = Split-Path -Parent $Current
    if ([string]::IsNullOrEmpty($Parent) -or $Parent -eq $Current) { break }
    $Current = $Parent
  }
}

function Assert-NotHardLink([string]$Path, [string]$Description) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return }
  $Item = Get-Item -LiteralPath $Path -Force
  $LinkTypeProperty = $Item.PSObject.Properties['LinkType']
  if (-not $LinkTypeProperty) {
    Fail "Unable to verify hard-link safety for $Description: $Path"
  }
  if ([string]$Item.LinkType -eq 'HardLink') {
    Fail "$Description must not be a hard link: $Path"
  }
}

function Initialize-SecureConfigPath([string]$Path) {
  $Directory = Split-Path -Parent $Path
  Assert-NoReparsePointAncestors $Directory 'Config directory'
  [IO.Directory]::CreateDirectory($Directory) | Out-Null
  Assert-NotReparsePoint $Directory 'Config directory'
  Protect-SensitiveConfigDirectory $Directory
  Assert-NotReparsePoint $Path 'Config file'
  if (Test-Path -LiteralPath $Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
      Fail "Config path is not a regular file: $Path"
    }
    Assert-NotHardLink $Path 'Config file'
    Protect-SensitiveConfigFile $Path
  }
}

function Write-SensitiveUtf8File([string]$Path, [string]$Content) {
  $CreateStream = [IO.File]::Open(
    $Path,
    [IO.FileMode]::CreateNew,
    [IO.FileAccess]::Write,
    [IO.FileShare]::None
  )
  $CreateStream.Dispose()
  Protect-SensitiveConfigFile $Path

  $Stream = [IO.File]::Open(
    $Path,
    [IO.FileMode]::Open,
    [IO.FileAccess]::Write,
    [IO.FileShare]::None
  )
  try {
    $Encoding = [Text.UTF8Encoding]::new($false)
    $Writer = [IO.StreamWriter]::new($Stream, $Encoding)
    try {
      $Writer.Write($Content)
      $Writer.Flush()
    } finally {
      $Writer.Dispose()
    }
  } finally {
    $Stream.Dispose()
  }
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
  $script:InstallDir = [IO.Path]::GetFullPath($script:InstallDir)
  $script:ConfigPath = [IO.Path]::GetFullPath($script:ConfigPath)
}

function Install-VerifiedTunnelBinary([string]$SourceBinary) {
  Assert-NoReparsePointAncestors $script:InstallDir 'Install directory'
  [IO.Directory]::CreateDirectory($script:InstallDir) | Out-Null
  Assert-NotReparsePoint $script:InstallDir 'Install directory'

  $TargetBinary = Join-Path $script:InstallDir 'aether-tunnel.exe'
  Assert-NotReparsePoint $TargetBinary 'Install target'
  if ((Test-Path -LiteralPath $TargetBinary) -and
      -not (Test-Path -LiteralPath $TargetBinary -PathType Leaf)) {
    Fail "Install target is not a regular file: $TargetBinary"
  }
  Assert-NotHardLink $TargetBinary 'Install target'

  $TempBinary = Join-Path $script:InstallDir ('.aether-tunnel.tmp.' + [Guid]::NewGuid().ToString('N'))
  try {
    [IO.File]::Copy($SourceBinary, $TempBinary, $false)
    Assert-NotReparsePoint $TargetBinary 'Install target'
    Assert-NotHardLink $TargetBinary 'Install target'
    if (Test-Path -LiteralPath $TargetBinary -PathType Leaf) {
      [IO.File]::Replace($TempBinary, $TargetBinary, $null, $true)
    } elseif (Test-Path -LiteralPath $TargetBinary) {
      Fail "Install target changed to a non-file during installation: $TargetBinary"
    } else {
      [IO.File]::Move($TempBinary, $TargetBinary)
    }
  } finally {
    if (Test-Path -LiteralPath $TempBinary) {
      Remove-Item -LiteralPath $TempBinary -Force
    }
  }
}

function Expand-VerifiedTunnelArchive([string]$Archive, [string]$Destination) {
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $Zip = [System.IO.Compression.ZipFile]::OpenRead($Archive)
  try {
    $Entries = @($Zip.Entries)
    if ($Entries.Count -ne 1) { Fail 'Release archive must contain exactly one file' }

    $Entry = $Entries[0]
    if (($Entry.FullName -ne 'aether-tunnel.exe') -or ($Entry.Name -ne 'aether-tunnel.exe')) {
      Fail 'Release archive must contain only aether-tunnel.exe at its root'
    }
    $UnixType = (($Entry.ExternalAttributes -shr 16) -band 0xF000)
    if (($UnixType -ne 0) -and ($UnixType -ne 0x8000)) {
      Fail 'aether-tunnel.exe in release archive is not a regular file'
    }
    if ($Entry.Length -le 0) { Fail 'aether-tunnel.exe in release archive is empty' }

    $InputStream = $Entry.Open()
    try {
      $OutputStream = [System.IO.File]::Open(
        $Destination,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
      )
      try {
        $InputStream.CopyTo($OutputStream)
      } finally {
        $OutputStream.Dispose()
      }
    } finally {
      $InputStream.Dispose()
    }
  } finally {
    $Zip.Dispose()
  }
}

function Install-AetherTunnelBinary([string]$Tag, [string]$TempDir) {
  Assert-SafeReleaseRepo $Repo
  Assert-SafeTunnelReleaseTag $Tag
  if (-not [Environment]::Is64BitOperatingSystem) { Fail 'Windows release currently supports amd64 only' }
  $Asset = 'aether-tunnel-windows-amd64.zip'
  $Base = "https://github.com/$Repo/releases/download/$Tag"
  $Archive = Join-Path $TempDir $Asset
  $Sums = Join-Path $TempDir 'SHA256SUMS.txt'

  Say "Downloading $Tag / $Asset"
  Save-TrustedGithubFile "$Base/$Asset" $Archive
  Save-TrustedGithubFile "$Base/SHA256SUMS.txt" $Sums
  if (-not (Test-Path -LiteralPath $Sums -PathType Leaf)) { Fail 'SHA256SUMS.txt download is missing' }

  $EscapedAsset = [regex]::Escape($Asset)
  $ChecksumTargetPattern = '(?:^|\s)\*?' + $EscapedAsset + '(?:\s|$)'
  $ExpectedLines = @(Get-Content -LiteralPath $Sums | Where-Object { $_ -match $ChecksumTargetPattern })
  if ($ExpectedLines.Count -ne 1) {
    Fail "SHA256SUMS.txt must contain exactly one entry for $Asset"
  }
  $ChecksumPattern = '^\s*(?<hash>\S+)\s+\*?' + $EscapedAsset + '\s*$'
  $ExpectedMatch = [regex]::Match([string]$ExpectedLines[0], $ChecksumPattern)
  if (-not $ExpectedMatch.Success) { Fail "SHA256SUMS.txt has an invalid entry for $Asset" }
  $Expected = $ExpectedMatch.Groups['hash'].Value
  if ($Expected -notmatch '^[0-9A-Fa-f]{64}$') { Fail "SHA256SUMS.txt has an invalid hash for $Asset" }

  $Actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Archive).Hash.ToLowerInvariant()
  if ($Actual -ne $Expected.ToLowerInvariant()) { Fail "SHA256 verification failed for $Asset" }

  $ExtractDir = Join-Path $TempDir 'extract'
  New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
  $Binary = Join-Path $ExtractDir 'aether-tunnel.exe'
  Expand-VerifiedTunnelArchive $Archive $Binary
  Install-VerifiedTunnelBinary $Binary
  Say "Installed binary: $(Join-Path $script:InstallDir 'aether-tunnel.exe')"
}

function Test-LegacySingleServerConfig([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return $false }
  foreach ($Line in Get-Content -LiteralPath $Path) {
    if ($Line -match '^\s*\[') { return $false }
    if ($Line -match '^\s*(aether_url|management_token)\s*=') { return $true }
  }
  return $false
}

function Test-ServerExists([string]$Path, [string]$QuotedUrl, [string]$QuotedName) {
  if (-not (Test-Path -LiteralPath $Path)) { return $false }
  $FoundUrl = $false
  $FoundName = $false
  foreach ($Line in Get-Content -LiteralPath $Path) {
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
  Assert-SafeNodeName $NodeName
  Initialize-SecureConfigPath $script:ConfigPath

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

  $ConfigExists = Test-Path -LiteralPath $script:ConfigPath -PathType Leaf
  $ExistingContent = if ($ConfigExists) {
    [IO.File]::ReadAllText($script:ConfigPath)
  } else {
    ''
  }
  if ($ConfigExists) {
    $BackupPath = "$script:ConfigPath.bak.$(Get-Date -Format yyyyMMddHHmmss).$([Guid]::NewGuid().ToString('N'))"
    Write-SensitiveUtf8File $BackupPath $ExistingContent
  }

  $Prefix = if ($ExistingContent.Length -gt 0) { "`n" } else { '' }
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
  $ConfigDir = Split-Path -Parent $script:ConfigPath
  $TempPath = Join-Path $ConfigDir ("." + (Split-Path -Leaf $script:ConfigPath) + ".tmp." + [Guid]::NewGuid().ToString('N'))
  try {
    Write-SensitiveUtf8File $TempPath ($ExistingContent + $Block + "`n")
    Assert-NotReparsePoint $script:ConfigPath 'Config file'
    if ($ConfigExists) {
      if (-not (Test-Path -LiteralPath $script:ConfigPath -PathType Leaf)) {
        Fail "Config file changed while it was being updated: $script:ConfigPath"
      }
      [IO.File]::Replace($TempPath, $script:ConfigPath, $null, $true)
    } else {
      [IO.File]::Move($TempPath, $script:ConfigPath)
    }
  } finally {
    if (Test-Path -LiteralPath $TempPath) {
      Remove-Item -LiteralPath $TempPath -Force
    }
  }
  Protect-SensitiveConfigArtifacts $script:ConfigPath
  Say "Appended [[servers]] to: $script:ConfigPath"
}

function Main {
  Assert-SafeReleaseRepo $Repo
  Initialize-Paths
  $AetherUrl = Prompt-IfEmpty 'AETHER_TUNNEL_AETHER_URL' $env:AETHER_TUNNEL_AETHER_URL 'Aether URL'
  $ManagementToken = Prompt-IfEmpty 'AETHER_TUNNEL_MANAGEMENT_TOKEN' $env:AETHER_TUNNEL_MANAGEMENT_TOKEN 'Management token (ae_xxx)'
  $NodeName = Prompt-IfEmpty 'AETHER_TUNNEL_NODE_NAME' $env:AETHER_TUNNEL_NODE_NAME 'Node name'
  Assert-SafeNodeName $NodeName
  $TunnelSecurity = if ($env:AETHER_TUNNEL_SECURITY) { $env:AETHER_TUNNEL_SECURITY } else { '' }
  $TunnelEncryptionKey = if ($env:AETHER_TUNNEL_ENCRYPTION_KEY) { $env:AETHER_TUNNEL_ENCRYPTION_KEY } else { '' }
  if ($TunnelSecurity -and ($TunnelSecurity -notin @('off', 'non_tls_required'))) {
    Fail 'AETHER_TUNNEL_SECURITY must be off or non_tls_required'
  }
  if (($TunnelSecurity -eq 'non_tls_required') -and -not $TunnelEncryptionKey) {
    Fail 'AETHER_TUNNEL_ENCRYPTION_KEY is required when AETHER_TUNNEL_SECURITY=non_tls_required'
  }

  $TempDir = Join-Path ([IO.Path]::GetTempPath()) ("aether-tunnel-" + [Guid]::NewGuid().ToString('N'))
  Assert-NoReparsePointAncestors (Split-Path -Parent $TempDir) 'Temporary directory'
  if (Test-Path -LiteralPath $TempDir) { Fail "Secure temporary path already exists: $TempDir" }
  [IO.Directory]::CreateDirectory($TempDir) | Out-Null
  Assert-NotReparsePoint $TempDir 'Temporary directory'
  Protect-SensitiveConfigDirectory $TempDir
  try {
    $Tag = Resolve-LatestTunnelTag
    Assert-SafeTunnelReleaseTag $Tag
    Install-AetherTunnelBinary $Tag $TempDir
    Add-ServerConfig $AetherUrl $ManagementToken $NodeName $TunnelSecurity $TunnelEncryptionKey
  } finally {
    if (Test-Path -LiteralPath $TempDir) {
      Assert-NotReparsePoint $TempDir 'Temporary directory'
      Remove-Item -Recurse -Force -LiteralPath $TempDir -ErrorAction SilentlyContinue
    }
  }

  Say 'Complete. Start or configure the node with:'
  Say "  & '$(Join-Path $script:InstallDir 'aether-tunnel.exe')' setup '$script:ConfigPath'"
}

Main
