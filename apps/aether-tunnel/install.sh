#!/bin/sh
set -eu
umask 077

REPO="${AETHER_TUNNEL_RELEASE_REPO:-fawney19/Aether}"
TAG="${AETHER_TUNNEL_RELEASE_TAG:-}"
INSTALL_DIR="${AETHER_TUNNEL_INSTALL_DIR:-}"
CONFIG_PATH="${AETHER_TUNNEL_CONFIG:-}"
TMP_DIR=""
CONFIG_TMP_PATH=""

say() { printf '%s\n' "[Aether Tunnel] $1"; }
fail() { printf '%s\n' "[Aether Tunnel] $1" >&2; exit 1; }

cleanup() {
  if [ -n "$CONFIG_TMP_PATH" ] && [ -f "$CONFIG_TMP_PATH" ] && [ ! -L "$CONFIG_TMP_PATH" ]; then
    rm -f "$CONFIG_TMP_PATH"
  fi
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT INT TERM

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "缺少命令：$1"
}

validate_https_download_url() {
  url="$1"
  case "$url" in
    https://*) ;;
    *) fail "远程下载必须使用绝对 HTTPS URL" ;;
  esac
  case "$url" in
    *'#'*) fail "远程下载 URL 不得包含 fragment" ;;
  esac
  authority=${url#https://}
  authority=${authority%%[/?]*}
  [ -n "$authority" ] || fail "远程下载 URL 的 host 不能为空"
  case "$authority" in
    *'@'*) fail "远程下载 URL 不得包含凭据" ;;
  esac
}

validate_trusted_github_download_url() {
  url="$1"
  validate_https_download_url "$url"
  authority=${url#https://}
  authority=${authority%%[/?]*}
  case "$authority" in
    *:*) fail "GitHub 下载 URL 不得使用非标准端口：$authority" ;;
  esac
  host=$(printf '%s' "$authority" | tr 'A-Z' 'a-z')
  case "$host" in
    api.github.com|github.com|objects.githubusercontent.com|*.objects.githubusercontent.com|release-assets.githubusercontent.com|*.release-assets.githubusercontent.com) ;;
    *) fail "GitHub 下载重定向到了不受信任的主机：$host" ;;
  esac
}

download() {
  url="$1"
  out="$2"
  validate_trusted_github_download_url "$url"
  command -v curl >/dev/null 2>&1 || fail "需要 curl 安全下载 GitHub release 制品"

  download_dir=$(dirname "$out")
  [ -d "$download_dir" ] && [ ! -L "$download_dir" ] \
    || fail "下载目标目录不是安全目录：$download_dir"
  current_url="$url"
  redirect_count=0
  while :; do
    response_tmp=$(mktemp "$download_dir/.aether-download.body.XXXXXXXX") \
      || fail "无法创建安全下载临时文件"
    headers_tmp=$(mktemp "$download_dir/.aether-download.headers.XXXXXXXX") || {
      rm -f "$response_tmp"
      fail "无法创建安全响应头临时文件"
    }
    if ! status=$(curl -sS --retry 3 --connect-timeout 10 \
      --proto '=https' --max-redirs 0 --dump-header "$headers_tmp" \
      --output "$response_tmp" --write-out '%{http_code}' "$current_url"); then
      rm -f "$response_tmp" "$headers_tmp"
      fail "GitHub 下载失败：$current_url"
    fi
    case "$status" in
      2??)
        rm -f "$headers_tmp"
        [ -f "$response_tmp" ] && [ ! -L "$response_tmp" ] \
          || fail "下载结果不是普通文件"
        mv -f "$response_tmp" "$out" || {
          rm -f "$response_tmp"
          fail "无法原子保存下载结果：$out"
        }
        return
        ;;
      301|302|303|307|308)
        location=$(awk '
          tolower(substr($0, 1, 9)) == "location:" {
            value = substr($0, 10)
            sub(/^[[:space:]]*/, "", value)
            sub(/\r$/, "", value)
          }
          END { print value }
        ' "$headers_tmp")
        rm -f "$response_tmp" "$headers_tmp"
        [ -n "$location" ] || fail "GitHub 重定向缺少 Location 响应头"
        validate_trusted_github_download_url "$location"
        redirect_count=$((redirect_count + 1))
        [ "$redirect_count" -le 10 ] || fail "GitHub 下载重定向次数过多"
        current_url="$location"
        ;;
      *)
        rm -f "$response_tmp" "$headers_tmp"
        fail "GitHub 下载返回 HTTP $status：$current_url"
        ;;
    esac
  done
}

validate_release_repo() {
  value="$1"
  [ -n "$value" ] && [ "${#value}" -le 200 ] \
    || fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符"
  case "$value" in
    */*) ;;
    *) fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符" ;;
  esac
  owner=${value%%/*}
  name=${value#*/}
  case "$name" in
    */*) fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符" ;;
  esac
  case "$owner" in
    [A-Za-z0-9]*) ;;
    *) fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符" ;;
  esac
  case "$name" in
    [A-Za-z0-9]*) ;;
    *) fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符" ;;
  esac
  case "$owner$name" in
    *[!A-Za-z0-9._-]*) fail "release 仓库必须是安全的 GitHub OWNER/REPO 标识符" ;;
  esac
}

validate_tunnel_release_tag() {
  value="$1"
  [ -n "$value" ] && [ "${#value}" -le 128 ] \
    || fail "release tag 包含不安全的 URL 或路径字符"
  case "$value" in
    *[!A-Za-z0-9._+-]*) fail "release tag 包含不安全的 URL 或路径字符" ;;
  esac
  version=${value#tunnel-v}
  [ "$version" != "$value" ] || fail "release tag 必须使用 tunnel-v<version> 格式"
  semver_identifier='(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
  if ! printf '%s' "$version" | LC_ALL=C grep -Eq "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(-(${semver_identifier})(\\.${semver_identifier})*)?(\\+[0-9A-Za-z-]+(\\.[0-9A-Za-z-]+)*)?$"; then
    fail "release tag 必须包含有效的 SemVer 版本"
  fi
}

validate_release_asset_name() {
  value="$1"
  [ -n "$value" ] && [ "${#value}" -le 200 ] \
    || fail "release 制品名称无效"
  case "$value" in
    aether-tunnel-*.tar.gz) ;;
    *) fail "release 制品名称无效" ;;
  esac
  case "$value" in
    *[!A-Za-z0-9._-]*) fail "release 制品名称包含不安全的路径字符" ;;
  esac
}

validate_node_name() {
  value="$1"
  [ -n "$value" ] && [ "${#value}" -le 255 ] \
    || fail "node name 必须是 1 到 255 个字符"
  newline='
'
  carriage_return=$(printf '\r')
  case "$value" in
    *"$newline"*|*"$carriage_return"*) fail "node name 不得包含控制字符" ;;
  esac
  case "$value" in
    [[:space:]]*|*[[:space:]]) fail "node name 不得包含首尾空白" ;;
  esac
  if printf '%s' "$value" | LC_ALL=C grep -q '[[:cntrl:]]'; then
    fail "node name 不得包含控制字符"
  fi
}

prompt_if_empty() {
  name="$1"
  value="$2"
  prompt="$3"
  if [ -n "$value" ]; then
    printf '%s' "$value"
    return
  fi
  printf '%s' "$prompt" >&2
  if [ -r /dev/tty ]; then
    IFS= read -r value < /dev/tty
  else
    fail "$name 未通过环境变量提供，且当前环境无法交互输入"
  fi
  [ -n "$value" ] || fail "$name 不能为空"
  printf '%s' "$value"
}

toml_quote() {
  value="$1"
  if command -v python3 >/dev/null 2>&1; then
    quoted=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1], ensure_ascii=False))' "$value" 2>/dev/null || true)
    if [ -n "$quoted" ]; then
      printf '%s\n' "$quoted"
      return
    fi
  fi
  printf '%s' "$value" | awk '
    BEGIN { printf "\"" }
    {
      if (NR > 1) printf "\\n"
      gsub(/\\/, "\\\\")
      gsub(/\"/, "\\\"")
      gsub(/\t/, "\\t")
      gsub(/\r/, "\\r")
      printf "%s", $0
    }
    END { print "\"" }
  '
}

resolve_latest_tunnel_tag() {
  validate_release_repo "$REPO"
  if [ -n "$TAG" ]; then
    validate_tunnel_release_tag "$TAG"
    printf '%s\n' "$TAG"
    return
  fi
  api_url="https://api.github.com/repos/${REPO}/releases?per_page=100"
  releases="$TMP_DIR/releases.json"
  download "$api_url" "$releases" >/dev/null 2>&1 || fail "无法读取 GitHub Releases：$api_url"
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$releases" <<'PY'
import json, re, sys
releases = json.load(open(sys.argv[1], encoding='utf-8'))
tag_pattern = re.compile(r'^tunnel-v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$')
tunnel = [r for r in releases if not r.get('draft') and not r.get('prerelease') and tag_pattern.fullmatch(str(r.get('tag_name', '')))]
tunnel.sort(key=lambda r: r.get('published_at') or r.get('created_at') or '', reverse=True)
if tunnel:
    print(tunnel[0]['tag_name'])
PY
  else
    # Without a JSON parser, only accept an exact stable SemVer tag.  This
    # keeps the fallback from accidentally selecting a prerelease entry.
    grep -Eo '"tag_name"[[:space:]]*:[[:space:]]*"tunnel-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"' "$releases" |
      head -n 1 |
      sed 's/.*"\(tunnel-v[^"]*\)".*/\1/'
  fi
}

detect_asset() {
  os=$(uname -s 2>/dev/null || printf unknown)
  arch=$(uname -m 2>/dev/null || printf unknown)

  case "$os" in
    Linux) platform=linux ;;
    Darwin) platform=macos ;;
    MINGW*|MSYS*|CYGWIN*) fail "检测到 Windows shell，请使用 PowerShell：irm <install.ps1-url> | iex" ;;
    *) fail "不支持的系统：$os" ;;
  esac

  case "$arch" in
    x86_64|amd64) cpu=amd64 ;;
    aarch64|arm64) cpu=arm64 ;;
    *) fail "不支持的 CPU 架构：$arch" ;;
  esac

  if [ "$platform" = "linux" ] && command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    printf 'aether-tunnel-linux-musl-%s.tar.gz\n' "$cpu"
  else
    printf 'aether-tunnel-%s-%s.tar.gz\n' "$platform" "$cpu"
  fi
}

choose_paths() {
  if [ -z "$INSTALL_DIR" ]; then
    if [ "$(id -u 2>/dev/null || printf 1)" = "0" ]; then
      INSTALL_DIR="/usr/local/bin"
    else
      INSTALL_DIR="$HOME/.local/bin"
    fi
  fi
  if [ -z "$CONFIG_PATH" ]; then
    if [ "$(id -u 2>/dev/null || printf 1)" = "0" ]; then
      CONFIG_PATH="/etc/aether-tunnel/aether-tunnel.toml"
    else
      CONFIG_PATH="$HOME/.aether-tunnel/aether-tunnel.toml"
    fi
  fi
}

stat_owner_id() {
  stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1" 2>/dev/null
}

stat_mode() {
  stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null
}

stat_link_count() {
  stat -c '%h' "$1" 2>/dev/null || stat -f '%l' "$1" 2>/dev/null
}

require_single_link_regular_file() {
  path="$1"
  description="$2"
  [ -f "$path" ] && [ ! -L "$path" ] || fail "$description 不是普通文件：$path"
  link_count=$(stat_link_count "$path") || fail "无法读取 $description 的硬链接计数：$path"
  [ "$link_count" = "1" ] || fail "$description 不得是多重硬链接文件：$path"
}

validate_trusted_directory_ancestors() {
  path="$1"
  description="$2"
  current="$path"
  while :; do
    [ ! -L "$current" ] || fail "$description 的祖先目录不得是符号链接：$current"
    if [ -e "$current" ]; then
      [ -d "$current" ] || fail "$description 的祖先路径不是目录：$current"
      ancestor_owner=$(stat_owner_id "$current") \
        || fail "无法读取 $description 的祖先目录所有者：$current"
      current_uid=$(id -u)
      [ "$ancestor_owner" = "0" ] || [ "$ancestor_owner" = "$current_uid" ] \
        || fail "$description 的祖先目录必须属于 root 或当前用户：$current"
      ancestor_mode=$(stat_mode "$current") \
        || fail "无法读取 $description 的祖先目录权限：$current"
      case "$ancestor_mode" in
        *[!0-7]*) fail "$description 的祖先目录权限格式无效：$ancestor_mode" ;;
      esac
      ancestor_permissions=$((0$ancestor_mode))
      [ $((ancestor_permissions & 0022)) -eq 0 ] \
        || fail "$description 的祖先目录不得允许组或其他用户写入：$current"
    fi
    parent=$(dirname "$current")
    [ "$parent" != "$current" ] || break
    current="$parent"
  done
}

prepare_secure_config_path() {
  config_dir=$(dirname "$CONFIG_PATH")
  config_name=$(basename "$CONFIG_PATH")
  validate_trusted_directory_ancestors "$config_dir" "配置目录"
  [ ! -L "$config_dir" ] || fail "配置目录不得是符号链接：$config_dir"
  if [ -e "$config_dir" ] && [ ! -d "$config_dir" ]; then
    fail "配置目录路径不是目录：$config_dir"
  fi
  if [ ! -d "$config_dir" ]; then
    mkdir -p "$config_dir"
    chmod 700 "$config_dir"
  fi
  [ ! -L "$config_dir" ] || fail "配置目录不得是符号链接：$config_dir"
  config_dir=$(cd "$config_dir" && pwd -P) || fail "无法解析配置目录：$config_dir"
  CONFIG_PATH="$config_dir/$config_name"

  config_dir_owner=$(stat_owner_id "$config_dir") || fail "无法读取配置目录所有者：$config_dir"
  [ "$config_dir_owner" = "$(id -u)" ] || fail "配置目录必须属于当前用户：$config_dir"
  config_dir_mode=$(stat_mode "$config_dir") || fail "无法读取配置目录权限：$config_dir"
  case "$config_dir_mode" in
    *[!0-7]*) fail "配置目录权限格式无效：$config_dir_mode" ;;
  esac
  config_dir_permissions=$((0$config_dir_mode))
  [ $((config_dir_permissions & 0022)) -eq 0 ] \
    || fail "配置目录不得允许组或其他用户写入：$config_dir"

  [ ! -L "$CONFIG_PATH" ] || fail "配置文件不得是符号链接：$CONFIG_PATH"
  if [ -e "$CONFIG_PATH" ]; then
    require_single_link_regular_file "$CONFIG_PATH" "配置文件"
    config_owner=$(stat_owner_id "$CONFIG_PATH") || fail "无法读取配置文件所有者：$CONFIG_PATH"
    [ "$config_owner" = "$(id -u)" ] || fail "配置文件必须属于当前用户：$CONFIG_PATH"
    chmod 600 "$CONFIG_PATH" || fail "无法保护配置文件权限：$CONFIG_PATH"
  fi
}

verify_checksum() {
  archive="$1"
  sums="$2"
  asset="$3"
  require_single_link_regular_file "$archive" "release 制品"
  require_single_link_regular_file "$sums" "SHA256 校验文件"
  matches=$(awk -v asset="$asset" '
    {
      targets_asset = 0
      for (field = 2; field <= NF; field += 1) {
        if ($field == asset || $field == "*" asset) targets_asset = 1
      }
      if (targets_asset) {
        if (NF == 2 && ($2 == asset || $2 == "*" asset)) print $1
        else print "INVALID"
      }
    }
  ' "$sums")
  match_count=$(printf '%s\n' "$matches" | awk 'NF { count += 1 } END { print count + 0 }')
  [ "$match_count" -eq 1 ] \
    || fail "SHA256SUMS.txt 必须且只能包含一个制品条目：$asset"
  expected=$(printf '%s\n' "$matches" | awk 'NF { print; exit }')
  [ "${#expected}" -eq 64 ] || fail "SHA256SUMS.txt 中的哈希格式无效：$asset"
  case "$expected" in
    *[!0-9A-Fa-f]*)
      fail "SHA256SUMS.txt 中的哈希格式无效：$asset"
      ;;
  esac
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$archive" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$archive" | awk '{print $1}')
  else
    fail "缺少 sha256sum 或 shasum，无法验证 release 制品"
  fi
  actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
  expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
  [ "$actual" = "$expected" ] || fail "SHA256 校验失败：$asset"
}

extract_tunnel_binary() {
  archive="$1"
  destination="$2"
  require_single_link_regular_file "$archive" "release 制品"
  members=$(tar -tzf "$archive" 2>/dev/null) || fail "无法读取 release 制品"
  member_count=$(printf '%s\n' "$members" | awk 'NF { count += 1 } END { print count + 0 }')
  [ "$member_count" -eq 1 ] || fail "制品必须且只能包含一个 aether-tunnel 文件"
  [ "$members" = "aether-tunnel" ] || fail "制品必须在根目录包含 aether-tunnel"

  listing=$(tar -tvzf "$archive" 2>/dev/null) || fail "无法读取 release 制品元数据"
  member_type=$(printf '%s\n' "$listing" | awk 'NF { print substr($1, 1, 1); exit }')
  [ "$member_type" = "-" ] || fail "制品中的 aether-tunnel 不是普通文件"

  destination_dir=$(dirname "$destination")
  extract_tmp=$(mktemp "$destination_dir/.aether-tunnel.extract.XXXXXXXX") \
    || fail "无法创建安全解压临时文件"
  if ! tar -xOzf "$archive" aether-tunnel > "$extract_tmp"; then
    rm -f "$extract_tmp"
    fail "无法安全提取 aether-tunnel"
  fi
  [ -s "$extract_tmp" ] || {
    rm -f "$extract_tmp"
    fail "制品中的 aether-tunnel 为空"
  }
  mv -f "$extract_tmp" "$destination" || {
    rm -f "$extract_tmp"
    fail "无法原子保存解压后的 aether-tunnel"
  }
  require_single_link_regular_file "$destination" "解压后的 aether-tunnel"
}

prepare_secure_install_dir() {
  validate_trusted_directory_ancestors "$INSTALL_DIR" "安装目录"
  [ ! -L "$INSTALL_DIR" ] || fail "安装目录不得是符号链接：$INSTALL_DIR"
  if [ -e "$INSTALL_DIR" ] && [ ! -d "$INSTALL_DIR" ]; then
    fail "安装目录路径不是目录：$INSTALL_DIR"
  fi
  if [ ! -d "$INSTALL_DIR" ]; then
    mkdir -p "$INSTALL_DIR"
  fi
  [ ! -L "$INSTALL_DIR" ] || fail "安装目录不得是符号链接：$INSTALL_DIR"
  INSTALL_DIR=$(cd "$INSTALL_DIR" && pwd -P) || fail "无法解析安装目录：$INSTALL_DIR"

  install_dir_owner=$(stat_owner_id "$INSTALL_DIR") || fail "无法读取安装目录所有者：$INSTALL_DIR"
  [ "$install_dir_owner" = "$(id -u)" ] || fail "安装目录必须属于当前用户：$INSTALL_DIR"
  install_dir_mode=$(stat_mode "$INSTALL_DIR") || fail "无法读取安装目录权限：$INSTALL_DIR"
  case "$install_dir_mode" in
    *[!0-7]*) fail "安装目录权限格式无效：$install_dir_mode" ;;
  esac
  install_dir_permissions=$((0$install_dir_mode))
  [ $((install_dir_permissions & 0022)) -eq 0 ] \
    || fail "安装目录不得允许组或其他用户写入：$INSTALL_DIR"
}

install_tunnel_binary_file() {
  source_binary="$1"
  require_single_link_regular_file "$source_binary" "待安装二进制"
  prepare_secure_install_dir
  target_binary="$INSTALL_DIR/aether-tunnel"
  [ ! -L "$target_binary" ] || fail "安装目标不得是符号链接：$target_binary"
  if [ -e "$target_binary" ]; then
    require_single_link_regular_file "$target_binary" "安装目标"
    target_owner=$(stat_owner_id "$target_binary") || fail "无法读取安装目标所有者：$target_binary"
    [ "$target_owner" = "$(id -u)" ] || fail "安装目标必须属于当前用户：$target_binary"
  fi

  install_tmp=$(mktemp "$INSTALL_DIR/.aether-tunnel.tmp.XXXXXXXX") \
    || fail "无法在安装目录中创建安全临时文件"
  if ! cat "$source_binary" > "$install_tmp"; then
    rm -f "$install_tmp"
    fail "无法写入临时安装文件"
  fi
  chmod 0755 "$install_tmp" || {
    rm -f "$install_tmp"
    fail "无法设置临时安装文件权限"
  }

  [ ! -L "$target_binary" ] || {
    rm -f "$install_tmp"
    fail "安装目标在写入期间变成了符号链接：$target_binary"
  }
  if [ -e "$target_binary" ]; then
    if ! require_single_link_regular_file "$target_binary" "安装目标"; then
      rm -f "$install_tmp"
      fail "安装目标在写入期间变得不安全：$target_binary"
    fi
  fi
  mv -f "$install_tmp" "$target_binary" || {
    rm -f "$install_tmp"
    fail "无法原子替换安装目标：$target_binary"
  }
}

install_binary() {
  tag="$1"
  asset="$2"
  validate_release_repo "$REPO"
  validate_tunnel_release_tag "$tag"
  validate_release_asset_name "$asset"
  base="https://github.com/${REPO}/releases/download/${tag}"
  archive="$TMP_DIR/$asset"
  say "下载 $tag / $asset"
  download "$base/$asset" "$archive"
  download "$base/SHA256SUMS.txt" "$TMP_DIR/SHA256SUMS.txt" >/dev/null 2>&1 || fail "无法下载 SHA256SUMS.txt"
  verify_checksum "$archive" "$TMP_DIR/SHA256SUMS.txt" "$asset"

  extract_tunnel_binary "$archive" "$TMP_DIR/aether-tunnel"
  install_tunnel_binary_file "$TMP_DIR/aether-tunnel"
  say "已安装二进制：$INSTALL_DIR/aether-tunnel"
}

has_legacy_single_server_keys() {
  [ -f "$CONFIG_PATH" ] || return 1
  awk '
    /^[[:space:]]*\[/ { exit }
    /^[[:space:]]*(aether_url|management_token)[[:space:]]*=/ { found=1; exit }
    END { exit found ? 0 : 1 }
  ' "$CONFIG_PATH"
}

server_exists() {
  [ -f "$CONFIG_PATH" ] || return 1
  quoted_url="$1"
  quoted_name="$2"
  awk -v url="aether_url = $quoted_url" -v name="node_name = $quoted_name" '
    BEGIN { found_url=0; found_name=0 }
    /^\[\[servers\]\]/ {
      if (found_url && found_name) { found=1 }
      found_url=0; found_name=0
    }
    $0 == url { found_url=1 }
    $0 == name { found_name=1 }
    END { if (found_url && found_name) { found=1 }; exit found ? 0 : 1 }
  ' "$CONFIG_PATH"
}

append_server_config() {
  aether_url="$1"
  management_token="$2"
  node_name="$3"
  tunnel_security="$4"
  tunnel_encryption_key="$5"

  validate_node_name "$node_name"
  prepare_secure_config_path
  quoted_url=$(toml_quote "$aether_url")
  quoted_token=$(toml_quote "$management_token")
  quoted_name=$(toml_quote "$node_name")
  quoted_encryption_key=$(toml_quote "$tunnel_encryption_key")
  if [ -n "$tunnel_security" ]; then
    quoted_security=$(toml_quote "$tunnel_security")
  fi

  if has_legacy_single_server_keys; then
    fail "现有配置仍使用旧的顶层 aether_url/management_token，请先运行 aether-tunnel setup 迁移为 [[servers]] 后重试：$CONFIG_PATH"
  fi

  if server_exists "$quoted_url" "$quoted_name"; then
    say "配置中已存在相同 aether_url + node_name，跳过追加：$CONFIG_PATH"
    return
  fi

  config_existed=false
  if [ -f "$CONFIG_PATH" ]; then
    config_existed=true
    backup_path=$(mktemp "$CONFIG_PATH.bak.$(date +%Y%m%d%H%M%S).XXXXXXXX") \
      || fail "无法创建安全配置备份"
    chmod 600 "$backup_path" || fail "无法保护配置备份权限：$backup_path"
    cat "$CONFIG_PATH" > "$backup_path" || fail "无法备份配置文件：$CONFIG_PATH"
  fi

  CONFIG_TMP_PATH=$(mktemp "$CONFIG_PATH.tmp.XXXXXXXX") || fail "无法创建安全配置临时文件"
  chmod 600 "$CONFIG_TMP_PATH" || fail "无法保护配置临时文件权限"
  if [ "$config_existed" = true ]; then
    cat "$CONFIG_PATH" > "$CONFIG_TMP_PATH" || fail "无法读取现有配置：$CONFIG_PATH"
  fi
  {
    if [ -s "$CONFIG_TMP_PATH" ]; then
      printf '\n'
    fi
    printf '# Added by Aether Tunnel one-click installer. Existing config is preserved.\n'
    printf '[[servers]]\n'
    printf 'aether_url = %s\n' "$quoted_url"
    printf 'management_token = %s\n' "$quoted_token"
    printf 'node_name = %s\n' "$quoted_name"
    if [ -n "$tunnel_security" ]; then
      printf 'tunnel_security = %s\n' "$quoted_security"
    fi
    if [ -n "$tunnel_encryption_key" ]; then
      printf 'tunnel_encryption_key = %s\n' "$quoted_encryption_key"
    fi
  } >> "$CONFIG_TMP_PATH"
  chmod 600 "$CONFIG_TMP_PATH" || fail "无法保护配置临时文件权限"
  [ ! -L "$CONFIG_PATH" ] || fail "配置文件在写入期间变成了符号链接：$CONFIG_PATH"
  if [ -e "$CONFIG_PATH" ]; then
    require_single_link_regular_file "$CONFIG_PATH" "配置文件"
  fi
  mv -f "$CONFIG_TMP_PATH" "$CONFIG_PATH" || fail "无法原子替换配置文件：$CONFIG_PATH"
  CONFIG_TMP_PATH=""
  chmod 600 "$CONFIG_PATH" || fail "无法保护配置文件权限：$CONFIG_PATH"
  say "已追加 [[servers]] 到：$CONFIG_PATH"
}

main() {
  TMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t aether-tunnel)
  need_cmd tar
  validate_release_repo "$REPO"
  choose_paths

  aether_url=$(prompt_if_empty AETHER_TUNNEL_AETHER_URL "${AETHER_TUNNEL_AETHER_URL:-}" "Aether URL: ")
  management_token=$(prompt_if_empty AETHER_TUNNEL_MANAGEMENT_TOKEN "${AETHER_TUNNEL_MANAGEMENT_TOKEN:-}" "Management token (ae_xxx): ")
  node_name=$(prompt_if_empty AETHER_TUNNEL_NODE_NAME "${AETHER_TUNNEL_NODE_NAME:-}" "Node name: ")
  validate_node_name "$node_name"
  tunnel_security="${AETHER_TUNNEL_SECURITY:-}"
  tunnel_encryption_key="${AETHER_TUNNEL_ENCRYPTION_KEY:-}"
  case "$tunnel_security" in
    ""|off|non_tls_required) ;;
    *) fail "AETHER_TUNNEL_SECURITY 必须是 off 或 non_tls_required" ;;
  esac
  if [ "$tunnel_security" = "non_tls_required" ] && [ -z "$tunnel_encryption_key" ]; then
    fail "AETHER_TUNNEL_SECURITY=non_tls_required 时必须设置 AETHER_TUNNEL_ENCRYPTION_KEY"
  fi

  tag=$(resolve_latest_tunnel_tag)
  [ -n "$tag" ] || fail "没有找到可用的 tunnel-v* release"
  validate_tunnel_release_tag "$tag"
  asset=$(detect_asset)
  install_binary "$tag" "$asset"
  append_server_config "$aether_url" "$management_token" "$node_name" "$tunnel_security" "$tunnel_encryption_key"

  say "完成。运行以下命令启动/配置服务："
  say "  $INSTALL_DIR/aether-tunnel setup $CONFIG_PATH"
}

main "$@"
