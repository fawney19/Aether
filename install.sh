#!/usr/bin/env bash
set -euo pipefail

REPO="${AETHER_REPO:-fawney19/Aether}"
SOURCE_REF="${AETHER_SOURCE_REF:-main}"
SOURCE_REF_EXPLICIT="false"
if [[ -n "${AETHER_SOURCE_REF:-}" ]]; then
    SOURCE_REF_EXPLICIT="true"
fi
VERSION="${AETHER_VERSION:-}"
CHANNEL="${AETHER_CHANNEL:-stable}"
CHANNEL_EXPLICIT="false"
if [[ -n "${AETHER_CHANNEL:-}" ]]; then
    CHANNEL_EXPLICIT="true"
fi
MODE="${AETHER_INSTALL_MODE:-auto}"
INSTALL_ROOT_EXPLICIT="false"
if [[ -n "${INSTALL_ROOT:-}" ]]; then
    INSTALL_ROOT_EXPLICIT="true"
fi
INSTALL_ROOT="${INSTALL_ROOT:-/opt/aether}"
CONFIG_DIR="${CONFIG_DIR:-/etc/aether}"
COMPOSE_DIR="${AETHER_COMPOSE_DIR:-}"
COMPOSE_DIR_EXPLICIT="false"
if [[ -n "${AETHER_COMPOSE_DIR:-}" ]]; then
    COMPOSE_DIR_EXPLICIT="true"
fi
IMAGE_REPO="${AETHER_IMAGE_REPO:-ghcr.io/fawney19/aether}"
APP_IMAGE="${AETHER_APP_IMAGE:-}"
SERVICE_USER_EXPLICIT="false"
SERVICE_GROUP_EXPLICIT="false"
if [[ -n "${SERVICE_USER:-}" ]]; then
    SERVICE_USER_EXPLICIT="true"
fi
if [[ -n "${SERVICE_GROUP:-}" ]]; then
    SERVICE_GROUP_EXPLICIT="true"
fi
SERVICE_USER="${SERVICE_USER:-aether}"
SERVICE_GROUP="${SERVICE_GROUP:-aether}"
SERVICE_NAME="aether-gateway"
COMPOSE_RELEASE_BASE_DIR="/opt/aether"
COMPOSE_RELEASE_CURRENT_DIR="${COMPOSE_RELEASE_BASE_DIR}/current"
COMPOSE_RELEASE_FRONTEND_DIR="${COMPOSE_RELEASE_CURRENT_DIR}/frontend"
COMPOSE_RELEASE_LOG_DIR="${COMPOSE_RELEASE_BASE_DIR}/logs"
COMPOSE_LOG_DESTINATION_DEFAULT="stdout"
COMPOSE_LOG_FORMAT_DEFAULT="pretty"
COMPOSE_LOG_ROTATION_DEFAULT="daily"
COMPOSE_LOG_RETENTION_DAYS_DEFAULT="7"
COMPOSE_LOG_MAX_FILES_DEFAULT="30"
COMPOSE_APP_PORT_DEFAULT="8084"
COMPOSE_CLI=()
LAUNCHD_LABEL="${AETHER_LAUNCHD_LABEL:-com.aether.gateway}"
LAUNCHD_LOG_DIR="${AETHER_LAUNCHD_LOG_DIR:-/var/log/aether}"
ENV_TARGET="${CONFIG_DIR}/aether-gateway.env"
SYSTEMD_UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
LAUNCHD_PLIST_PATH="/Library/LaunchDaemons/${LAUNCHD_LABEL}.plist"
TMP_ROOT=""
ARCHIVE_PATH=""
BUNDLE_DIR=""
ENV_SOURCE=""
SKIP_START="false"
GENERATED_ENV=""
ADMIN_PASSWORD_SOURCE=""
UI_LANG="${AETHER_LANG:-${AETHER_LANGUAGE:-auto}}"
RELEASE_KEEP="${AETHER_RELEASE_KEEP:-3}"
RELEASE_ARCHIVE_URL="${AETHER_RELEASE_ARCHIVE_URL:-${AETHER_DOWNLOAD_URL:-}}"
MAX_RELEASE_ARCHIVE_ENTRIES=100000
MAX_RELEASE_UNPACKED_BYTES=$((2 * 1024 * 1024 * 1024))

usage() {
    cat <<'EOF'
Usage: install.sh [options]

Install Aether Gateway.

Options:
  --mode MODE          Deployment mode: compose, compose-single-node, or single-node
                      compose: Docker Compose app + Postgres + Redis
                      compose-single-node: Docker Compose single-node app
                      single-node: single-node system service
                      Linux services use systemd; macOS services use launchd
  --channel CHANNEL    Release channel to resolve when --version is omitted: stable, latest, rc, beta, or nightly
                      stable/latest resolves the latest stable tag (default)
                      rc resolves the latest tag like v0.7.0-rc.1
                      beta resolves the latest tag like v0.7.0-beta.1
                      nightly resolves the rolling nightly build from main
  --version VERSION    Exact release tag to install, for example v0.7.0-rc.1 or nightly
  --repo OWNER/REPO    GitHub repository to download from (default: fawney19/Aether)
  --source-ref REF     Source branch/tag used for compose templates (default: main)
  --archive PATH       Install from a local release tarball instead of downloading
  --download-url URL   Download the release archive from this URL instead of GitHub
  --env-file PATH      Use an existing aether-gateway.env file
  --install-root PATH  Install root for system service mode (default: /opt/aether)
                      Also makes the default Docker Compose directory PATH/compose
  --compose-dir PATH   Docker Compose deployment directory (default: current directory)
  --config-dir PATH    Config directory (default: /etc/aether)
  --lang LANG          Installer language: zh or en
  --skip-start         Install files, but do not start Docker Compose or restart the service
  --keep-releases N    Keep the latest N releases, prune older ones (default: 3, 0=disable)
  -h, --help           Show this help

Environment overrides:
  AETHER_REPO, AETHER_SOURCE_REF, AETHER_INSTALL_MODE, AETHER_CHANNEL, AETHER_VERSION
  AETHER_LANG or AETHER_LANGUAGE
  AETHER_RELEASE_ARCHIVE_URL or AETHER_DOWNLOAD_URL
  AETHER_LAUNCHD_LABEL, AETHER_LAUNCHD_LOG_DIR, AETHER_RELEASE_KEEP
  AETHER_IMAGE_REPO, AETHER_APP_IMAGE
  INSTALL_ROOT, AETHER_COMPOSE_DIR, CONFIG_DIR, SERVICE_USER, SERVICE_GROUP
  ADMIN_PASSWORD (required for non-interactive first install when generating a new env)
EOF
}

die() {
    if ui_is_zh; then
        echo "错误: $*" >&2
    else
        echo "ERROR: $*" >&2
    fi
    exit 1
}

info() {
    echo ">>> $*" >&2
}

warn() {
    if ui_is_zh; then
        echo "警告: $*" >&2
    else
        echo "WARNING: $*" >&2
    fi
}

ui_is_zh() {
    case "${UI_LANG}" in
        zh|zh-*|cn|chinese|Chinese|中文)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

interactive_tty_available() {
    [[ -r /dev/tty && -w /dev/tty ]]
}

normalize_ui_lang() {
    local value="$1"
    value="$(printf '%s' "${value}" | tr '[:upper:]' '[:lower:]')"
    case "${value}" in
        zh|zh-cn|cn|chinese|中文)
            echo "zh"
            ;;
        en|en-us|english|英语)
            echo "en"
            ;;
        auto|"")
            echo "auto"
            ;;
        *)
            die "unsupported installer language: ${value}; expected zh or en"
            ;;
    esac
}

select_language() {
    UI_LANG="$(normalize_ui_lang "${UI_LANG}")"
    if [[ "${UI_LANG}" != "auto" ]]; then
        return
    fi

    if interactive_tty_available; then
        cat >/dev/tty <<'EOF'

请选择安装语言 / Choose installer language:
  1) 中文
  2) English

请输入选项 / Enter choice [1]:
EOF
        local choice
        IFS= read -r choice </dev/tty || choice=""
        case "${choice:-1}" in
            1)
                UI_LANG="zh"
                ;;
            2)
                UI_LANG="en"
                ;;
            *)
                UI_LANG="zh"
                die "无效的语言选项: ${choice}"
                ;;
        esac
    else
        UI_LANG="en"
    fi
}

cleanup() {
    if [[ -n "${TMP_ROOT}" && -d "${TMP_ROOT}" ]]; then
        rm -rf "${TMP_ROOT}"
    fi
}
if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then
    trap cleanup EXIT
fi

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --mode)
                [[ $# -ge 2 ]] || die "--mode requires a value"
                MODE="$2"
                shift 2
                ;;
            --channel)
                [[ $# -ge 2 ]] || die "--channel requires a value"
                CHANNEL="$2"
                CHANNEL_EXPLICIT="true"
                shift 2
                ;;
            --version)
                [[ $# -ge 2 ]] || die "--version requires a value"
                VERSION="$2"
                shift 2
                ;;
            --repo)
                [[ $# -ge 2 ]] || die "--repo requires a value"
                REPO="$2"
                shift 2
                ;;
            --source-ref)
                [[ $# -ge 2 ]] || die "--source-ref requires a value"
                SOURCE_REF="$2"
                SOURCE_REF_EXPLICIT="true"
                shift 2
                ;;
            --archive)
                [[ $# -ge 2 ]] || die "--archive requires a path"
                ARCHIVE_PATH="$2"
                shift 2
                ;;
            --download-url|--archive-url|--release-url)
                [[ $# -ge 2 ]] || die "--download-url requires a value"
                RELEASE_ARCHIVE_URL="$2"
                shift 2
                ;;
            --env-file)
                [[ $# -ge 2 ]] || die "--env-file requires a path"
                ENV_SOURCE="$2"
                shift 2
                ;;
            --install-root)
                [[ $# -ge 2 ]] || die "--install-root requires a path"
                INSTALL_ROOT="$2"
                INSTALL_ROOT_EXPLICIT="true"
                shift 2
                ;;
            --compose-dir)
                [[ $# -ge 2 ]] || die "--compose-dir requires a path"
                COMPOSE_DIR="$2"
                COMPOSE_DIR_EXPLICIT="true"
                shift 2
                ;;
            --config-dir)
                [[ $# -ge 2 ]] || die "--config-dir requires a path"
                CONFIG_DIR="$2"
                ENV_TARGET="${CONFIG_DIR}/aether-gateway.env"
                shift 2
                ;;
            --lang|--language)
                [[ $# -ge 2 ]] || die "--lang requires a value"
                UI_LANG="$2"
                shift 2
                ;;
            --skip-start)
                SKIP_START="true"
                shift
                ;;
            --keep-releases)
                [[ $# -ge 2 ]] || die "--keep-releases requires a number"
                RELEASE_KEEP="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "unknown argument: $1"
                ;;
        esac
    done
}

install_os() {
    case "$(uname -s)" in
        Linux)
            echo "linux"
            ;;
        Darwin)
            echo "macos"
            ;;
        *)
            if ui_is_zh; then
                die "Aether 二进制安装仅支持 Linux 和 macOS"
            else
                die "Aether binary install is only supported on Linux and macOS"
            fi
            ;;
    esac
}

is_darwin() {
    [[ "$(install_os)" == "macos" ]]
}

apply_platform_defaults() {
    if is_darwin; then
        if [[ "${SERVICE_USER_EXPLICIT}" != "true" ]]; then
            SERVICE_USER="_aether"
        fi
        if [[ "${SERVICE_GROUP_EXPLICIT}" != "true" ]]; then
            SERVICE_GROUP="_aether"
        fi
    fi
}

require_supported_os() {
    install_os >/dev/null
}

require_root() {
    if [[ "${EUID}" -ne 0 ]]; then
        if ui_is_zh; then
            die "请使用 root 运行"
        else
            die "run as root"
        fi
    fi
}

require_systemd() {
    if ! command -v systemctl >/dev/null 2>&1; then
        if ui_is_zh; then
            die "未找到 systemctl"
        else
            die "systemctl not found"
        fi
    fi
}

require_launchd() {
    if ! command -v launchctl >/dev/null 2>&1; then
        if ui_is_zh; then
            die "未找到 launchctl"
        else
            die "launchctl not found"
        fi
    fi
}

require_service_manager() {
    case "$(install_os)" in
        linux)
            require_systemd
            ;;
        macos)
            require_launchd
            ;;
    esac
}

service_manager_name() {
    case "$(install_os)" in
        linux)
            echo "systemd"
            ;;
        macos)
            echo "launchd"
            ;;
    esac
}

select_version() {
    if [[ -n "${VERSION}" || -n "${ARCHIVE_PATH}" || "${CHANNEL_EXPLICIT}" == "true" ]]; then
        return
    fi

    if interactive_tty_available; then
        if ui_is_zh; then
            cat >/dev/tty <<'EOF'

请选择 Aether 版本:
  1) 最新正式版
  2) 最新 RC 预发布版
  3) 最新 Beta 预发布版
  4) 最新 nightly 构建版
  5) 指定 tag，例如 v0.7.0-rc.1

请输入选项 [1]:
EOF
        else
            cat >/dev/tty <<'EOF'

Choose Aether version:
  1) Latest stable release
  2) Latest RC prerelease
  3) Latest beta prerelease
  4) Latest nightly build
  5) Exact tag, for example v0.7.0-rc.1

Enter choice [1]:
EOF
        fi
        local choice
        IFS= read -r choice </dev/tty || choice=""
        case "${choice:-1}" in
            1)
                CHANNEL="stable"
                ;;
            2)
                CHANNEL="rc"
                ;;
            3)
                CHANNEL="beta"
                ;;
            4)
                CHANNEL="nightly"
                ;;
            5)
                if ui_is_zh; then
                    cat >/dev/tty <<'EOF'
请输入准确 tag:
EOF
                else
                    cat >/dev/tty <<'EOF'
Enter exact tag:
EOF
                fi
                IFS= read -r VERSION </dev/tty || VERSION=""
                if [[ -z "${VERSION}" ]]; then
                    if ui_is_zh; then
                        die "准确 tag 不能为空"
                    else
                        die "exact tag cannot be empty"
                    fi
                fi
                ;;
            *)
                if ui_is_zh; then
                    die "无效的版本选项: ${choice}"
                else
                    die "invalid version choice: ${choice}"
                fi
                ;;
        esac
    fi
}

is_safe_release_identifier() {
    local value="$1"
    [[ -n "${value}" && ${#value} -le 128 ]] || return 1
    [[ "${value}" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]]
}

validate_release_identifier() {
    local value="$1"
    is_safe_release_identifier "${value}" \
        || die "release version contains unsafe URL or filesystem characters"
}

validate_installer_source_identifiers() {
    [[ ${#REPO} -le 200 && "${REPO}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$ ]] \
        || die "repository must be a safe GitHub OWNER/REPO identifier"
    [[ -n "${SOURCE_REF}" && ${#SOURCE_REF} -le 240 ]] \
        || die "source ref must be a non-empty identifier of at most 240 characters"
    [[ "${SOURCE_REF}" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] \
        || die "source ref contains unsafe URL characters"
    case "/${SOURCE_REF}/" in
        *"//"*|*"/./"*|*"/../"*)
            die "source ref contains an unsafe path component"
            ;;
    esac
    if [[ -n "${VERSION}" ]]; then
        validate_release_identifier "${VERSION}"
    fi
}

validate_service_account_identifier() {
    local kind="$1"
    local value="$2"
    [[ -n "${value}" && ${#value} -le 64 ]] \
        || die "${kind} must be a non-empty account identifier of at most 64 characters"
    [[ "${value}" =~ ^[A-Za-z_][A-Za-z0-9_.-]*\$?$ ]] \
        || die "${kind} contains unsafe account-name characters"
}

validate_launchd_label() {
    local value="$1"
    [[ -n "${value}" && ${#value} -le 128 \
        && "${value}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] \
        || die "launchd label contains unsafe filesystem or service-name characters"
}

validate_managed_absolute_path() {
    local kind="$1"
    local value="$2"
    [[ "${value}" == /* && ${#value} -le 1024 ]] \
        || die "${kind} must be an absolute path of at most 1024 characters"
    [[ "${value}" != *$'\n'* && "${value}" != *$'\r'* ]] \
        || die "${kind} may not contain line breaks"
    [[ "${value}" =~ ^/[A-Za-z0-9._/+,-]+$ ]] \
        || die "${kind} contains characters unsafe for generated service files"
    case "/${value#/}/" in
        *"//"*|*"/./"*|*"/../"*)
            die "${kind} contains an unsafe path component"
            ;;
    esac
}

validate_single_node_managed_paths() {
    validate_service_account_identifier "service user" "${SERVICE_USER}"
    validate_service_account_identifier "service group" "${SERVICE_GROUP}"
    validate_managed_absolute_path "install root" "${INSTALL_ROOT}"
    validate_managed_absolute_path "config directory" "${CONFIG_DIR}"
    validate_managed_absolute_path "env target" "${ENV_TARGET}"
    validate_privileged_path_ancestor "${INSTALL_ROOT}"
    validate_privileged_path_ancestor "${CONFIG_DIR}"
    validate_managed_regular_file "${ENV_TARGET}" true
    case "$(install_os)" in
        linux)
            validate_managed_absolute_path "systemd unit path" "${SYSTEMD_UNIT_PATH}"
            validate_privileged_path_ancestor "${SYSTEMD_UNIT_PATH}"
            validate_managed_regular_file "${SYSTEMD_UNIT_PATH}" true
            ;;
        macos)
            validate_launchd_label "${LAUNCHD_LABEL}"
            validate_managed_absolute_path "launchd plist path" "${LAUNCHD_PLIST_PATH}"
            validate_managed_absolute_path "launchd log directory" "${LAUNCHD_LOG_DIR}"
            validate_privileged_path_ancestor "${LAUNCHD_PLIST_PATH}"
            validate_privileged_path_ancestor "${LAUNCHD_LOG_DIR}"
            validate_managed_regular_file "${LAUNCHD_PLIST_PATH}" true
            validate_managed_regular_file "$(launchd_wrapper_path)" true
            validate_managed_regular_file \
                "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log" false
            validate_managed_regular_file \
                "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log" false
            ;;
    esac
}

select_mode() {
    case "${MODE}" in
        compose|docker|docker-compose)
            MODE="compose"
            return
            ;;
        compose-single-node|docker-single-node|docker-single-node-compose)
            MODE="compose-single-node"
            return
            ;;
        single-node|service|systemd|launchd)
            MODE="single-node"
            return
            ;;
        cluster|multi|multi-node)
            if ui_is_zh; then
                die "集群部署模式暂未开放；请先选择 compose、compose-single-node 或 single-node"
            else
                die "cluster deployment mode is temporarily disabled; choose compose, compose-single-node, or single-node"
            fi
            ;;
        auto|"")
            ;;
        *)
            die "unsupported install mode: ${MODE}; expected compose, compose-single-node, or single-node"
            ;;
    esac

    if interactive_tty_available; then
        if ui_is_zh; then
            cat >/dev/tty <<EOF

请选择 Aether 部署模式:
  1) Docker Compose 标准部署（Postgres + Redis）
  2) Docker Compose 单节点部署（PostgreSQL）
  3) 系统服务单节点部署（需提供 PostgreSQL DATABASE_URL）

请输入选项 [3]:
EOF
        else
            cat >/dev/tty <<EOF

Choose Aether deployment mode:
  1) Docker Compose standard deployment (Postgres + Redis)
  2) Docker Compose single-node deployment (PostgreSQL)
  3) System service single-node deployment (PostgreSQL)

Enter choice [3]:
EOF
        fi
        local choice
        IFS= read -r choice </dev/tty || choice=""
        case "${choice:-3}" in
            1)
                MODE="compose"
                ;;
            2)
                MODE="compose-single-node"
                ;;
            3)
                MODE="single-node"
                ;;
            *)
                if ui_is_zh; then
                    die "无效的部署模式选项: ${choice}"
                else
                    die "invalid deployment mode choice: ${choice}"
                fi
                ;;
        esac
    else
        MODE="single-node"
    fi
}

prompt_admin_password() {
    if [[ -n "${ADMIN_PASSWORD:-}" ]]; then
        ADMIN_PASSWORD_SOURCE="environment"
        return
    fi

    if interactive_tty_available; then
        local password confirm
        while true; do
            if ui_is_zh; then
                printf '\n请输入初始管理员密码: ' >/dev/tty
            else
                printf '\nEnter initial admin password: ' >/dev/tty
            fi
            stty -echo </dev/tty
            IFS= read -r password </dev/tty || password=""
            stty echo </dev/tty
            if ui_is_zh; then
                printf '\n请再次输入初始管理员密码: ' >/dev/tty
            else
                printf '\nConfirm initial admin password: ' >/dev/tty
            fi
            stty -echo </dev/tty
            IFS= read -r confirm </dev/tty || confirm=""
            stty echo </dev/tty
            printf '\n' >/dev/tty

            [[ -n "${password}" ]] || {
                if ui_is_zh; then
                    echo "管理员密码不能为空。" >/dev/tty
                else
                    echo "Admin password cannot be empty." >/dev/tty
                fi
                continue
            }
            [[ "${password}" == "${confirm}" ]] || {
                if ui_is_zh; then
                    echo "两次输入的密码不一致。" >/dev/tty
                else
                    echo "Passwords did not match." >/dev/tty
                fi
                continue
            }
            ADMIN_PASSWORD="${password}"
            ADMIN_PASSWORD_SOURCE="prompt"
            return
        done
    fi

    if ui_is_zh; then
        die "非交互式安装生成新配置时必须设置 ADMIN_PASSWORD"
    else
        die "ADMIN_PASSWORD is required when installing without an interactive terminal"
    fi
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            echo "amd64"
            ;;
        aarch64|arm64)
            echo "arm64"
            ;;
        *)
            die "unsupported CPU architecture: $(uname -m)"
            ;;
    esac
}

validate_https_download_url() {
    local url="$1"
    local authority

    [[ "${url}" == https://* ]] || die "remote downloads require an absolute HTTPS URL"
    [[ "${url}" != *"#"* ]] || die "remote download URLs may not contain a fragment"
    authority="${url#https://}"
    authority="${authority%%[/?]*}"
    [[ -n "${authority}" && "${authority}" != *"@"* ]] \
        || die "remote download URLs may not contain credentials or an empty host"
}

download_to() {
    local url="$1"
    local output="$2"
    local mode="${3:-quiet}"
    local show_progress="false"
    command -v curl >/dev/null 2>&1 || die "curl is required for secure remote downloads"
    validate_https_download_url "${url}"
    if [[ "${mode}" == "progress" && -t 2 ]]; then
        show_progress="true"
    fi

    if [[ "${show_progress}" == "true" ]]; then
        curl -fL --proto '=https' --proto-redir '=https' --progress-bar "${url}" -o "${output}"
    else
        curl -fsSL --proto '=https' --proto-redir '=https' "${url}" -o "${output}"
    fi
}

download_stdout() {
    local url="$1"
    command -v curl >/dev/null 2>&1 || die "curl is required for secure remote downloads"
    validate_https_download_url "${url}"
    curl -fsSL --proto '=https' --proto-redir '=https' "${url}"
}

verify_release_checksum() {
    local archive="$1"
    local checksum_file="$2"
    local asset="$3"
    local expected actual matches

    [[ -f "${checksum_file}" ]] || die "release checksum manifest is missing"
    matches="$(awk -v asset="${asset}" '
        ($2 == asset || $2 == "*" asset) && $1 ~ /^[0-9A-Fa-f]{64}$/ {
            print tolower($1)
        }
    ' "${checksum_file}")"
    [[ "$(printf '%s\n' "${matches}" | awk 'NF { count += 1 } END { print count + 0 }')" -eq 1 ]] \
        || die "release checksum manifest must contain exactly one valid entry for ${asset}"
    expected="$(printf '%s\n' "${matches}" | awk 'NF { print; exit }')"

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "${archive}" | awk '{print tolower($1)}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "${archive}" | awk '{print tolower($1)}')"
    else
        die "sha256sum or shasum is required to verify release assets"
    fi
    [[ "${actual}" == "${expected}" ]] || die "SHA256 verification failed for ${asset}"
}

validate_release_archive() {
    local archive="$1"
    local expected_root="${2:-}"
    local members_file listing_file normalized_file root member normalized permissions mode type
    local entry_count size_field tar_version unpacked_bytes

    members_file="${TMP_ROOT}/archive-members.txt"
    listing_file="${TMP_ROOT}/archive-listing.txt"
    normalized_file="${TMP_ROOT}/archive-members-normalized.txt"
    LC_ALL=C tar -tzf "${archive}" >"${members_file}" 2>/dev/null \
        || die "release archive cannot be read"
    tar_version="$(tar --version 2>/dev/null | head -n1 || true)"
    case "${tar_version}" in
        *bsdtar*)
            size_field=5
            ;;
        *GNU\ tar*)
            size_field=3
            ;;
        *)
            die "unsupported tar implementation for safe release validation"
            ;;
    esac
    LC_ALL=C tar --numeric-owner -tvzf "${archive}" >"${listing_file}" 2>/dev/null \
        || die "release archive metadata cannot be read"
    [[ -s "${members_file}" ]] || die "release archive is empty"

    entry_count="$(wc -l <"${members_file}" | tr -d '[:space:]')"
    [[ "${entry_count}" =~ ^[0-9]+$ && "${entry_count}" -le "${MAX_RELEASE_ARCHIVE_ENTRIES}" ]] \
        || die "release archive contains too many entries"
    if [[ "${entry_count}" != "$(wc -l <"${listing_file}" | tr -d '[:space:]')" ]]; then
        die "release archive contains malformed member names"
    fi
    unpacked_bytes="$(awk -v size_field="${size_field}" -v max="${MAX_RELEASE_UNPACKED_BYTES}" '
        {
            size = $size_field
            if (size !~ /^[0-9]+$/ || size > max - total) {
                exit 1
            }
            total += size
        }
        END {
            if (total <= max) {
                print total
            }
        }
    ' "${listing_file}")" \
        || die "release archive has invalid sizes or exceeds the unpacked size limit"
    [[ "${unpacked_bytes}" =~ ^[0-9]+$ && "${unpacked_bytes}" -le "${MAX_RELEASE_UNPACKED_BYTES}" ]] \
        || die "release archive has invalid sizes or exceeds the unpacked size limit"
    while IFS= read -r permissions; do
        type="${permissions:0:1}"
        mode="${permissions:0:10}"
        [[ "${type}" == "-" || "${type}" == "d" ]] \
            || die "release archive may contain only regular files and directories"
        [[ "${mode}" != *[sStT]* ]] \
            || die "release archive contains unsafe special permissions"
        [[ "${mode:5:1}" != "w" && "${mode:8:1}" != "w" ]] \
            || die "release archive contains group- or world-writable members"
    done <"${listing_file}"

    : >"${normalized_file}"
    root=""
    while IFS= read -r member; do
        [[ -n "${member}" ]] || die "release archive contains an empty member name"
        [[ "${member}" != /* && "${member}" != *\\* ]] \
            || die "release archive contains an unsafe member path"
        [[ "${member}" =~ ^[A-Za-z0-9._/@%+=,-]+/?$ ]] \
            || die "release archive contains a member name with unsafe characters"

        normalized="${member%/}"
        [[ -n "${normalized}" ]] \
            || die "release archive contains an invalid member path"
        case "/${normalized}/" in
            *"//"*|*"/./"*|*"/../"*)
                die "release archive contains path traversal or an empty path component"
                ;;
        esac
        member="${normalized}"
        normalized="${member%%/*}"
        if [[ -z "${root}" ]]; then
            root="${normalized}"
        elif [[ "${normalized}" != "${root}" ]]; then
            die "release archive must contain exactly one top-level bundle directory"
        fi
        printf '%s\n' "${member}" >>"${normalized_file}"
    done <"${members_file}"

    [[ -n "${root}" ]] || die "release archive did not contain a bundle directory"
    if [[ -n "${expected_root}" && "${root}" != "${expected_root}" ]]; then
        die "release archive root ${root} does not match expected bundle ${expected_root}"
    fi
    [[ -z "$(LC_ALL=C sort "${normalized_file}" | uniq -d | head -n1)" ]] \
        || die "release archive contains duplicate members"
    printf '%s\n' "${root}"
}

extract_validated_release_archive() {
    local archive="$1"
    local -a tar_args=(-xzf "${archive}" -C "${TMP_ROOT}" --no-same-owner --no-acls --no-xattrs)
    if tar --version 2>/dev/null | head -n1 | grep -qi 'bsdtar'; then
        tar_args+=(--no-fflags --no-mac-metadata)
    elif tar --version 2>/dev/null | head -n1 | grep -qi 'gnu tar'; then
        tar_args+=(--no-selinux)
    fi
    tar "${tar_args[@]}" \
        || die "release archive extraction failed"
}

select_release_download_urls() {
    local original_archive_url="$1"

    if [[ -z "${RELEASE_ARCHIVE_URL}" && interactive_tty_available ]]; then
        if ui_is_zh; then
            cat >/dev/tty <<'EOF'

是否使用下载加速源?
  1) 否，使用原始 GitHub 地址
  2) 是，手动填写新的下载 URL

请输入选项 [1]:
EOF
        else
            cat >/dev/tty <<'EOF'

Use an accelerated download URL?
  1) No, use the original GitHub URL
  2) Yes, enter a replacement download URL

Enter choice [1]:
EOF
        fi

        local choice
        IFS= read -r choice </dev/tty || choice=""
        case "${choice:-1}" in
            1)
                ;;
            2)
                if ui_is_zh; then
                    cat >/dev/tty <<EOF

原始压缩包 URL:
  ${original_archive_url}

请输入新的压缩包下载 URL:
EOF
                else
                    cat >/dev/tty <<EOF

Original archive URL:
  ${original_archive_url}

Enter replacement archive download URL:
EOF
                fi
                IFS= read -r RELEASE_ARCHIVE_URL </dev/tty || RELEASE_ARCHIVE_URL=""
                [[ -n "${RELEASE_ARCHIVE_URL}" ]] || {
                    if ui_is_zh; then
                        die "新的压缩包下载 URL 不能为空"
                    else
                        die "replacement archive download URL cannot be empty"
                    fi
                }
                ;;
            *)
                if ui_is_zh; then
                    die "无效的下载源选项: ${choice}"
                else
                    die "invalid download source choice: ${choice}"
                fi
                ;;
        esac
    fi

    if [[ -z "${RELEASE_ARCHIVE_URL}" ]]; then
        RELEASE_ARCHIVE_URL="${original_archive_url}"
    elif [[ "${RELEASE_ARCHIVE_URL}" != "${original_archive_url}" ]]; then
        if ui_is_zh; then
            info "使用自定义压缩包下载 URL"
            info "原始压缩包 URL: ${original_archive_url}"
        else
            info "using custom archive download URL"
            info "original archive URL: ${original_archive_url}"
        fi
    fi
}

raw_project_url() {
    local path="$1"
    printf 'https://raw.githubusercontent.com/%s/%s/%s' "${REPO}" "${SOURCE_REF}" "${path}"
}

same_path() {
    local left="$1"
    local right="$2"
    local left_dir right_dir left_base right_base

    [[ -e "${left}" && -e "${right}" ]] || return 1

    left_dir="$(cd -- "$(dirname -- "${left}")" && pwd -P)"
    right_dir="$(cd -- "$(dirname -- "${right}")" && pwd -P)"
    left_base="$(basename -- "${left}")"
    right_base="$(basename -- "${right}")"

    [[ "${left_dir}/${left_base}" == "${right_dir}/${right_base}" ]]
}

stat_file_mode() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null
}

stat_file_uid() {
    stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1" 2>/dev/null
}

stat_file_gid() {
    stat -c '%g' "$1" 2>/dev/null || stat -f '%g' "$1" 2>/dev/null
}

stat_file_link_count() {
    stat -c '%h' "$1" 2>/dev/null || stat -f '%l' "$1" 2>/dev/null
}

validate_managed_regular_file() {
    local path="$1"
    local allow_hardlinks="${2:-true}"

    [[ ! -L "${path}" ]] || die "managed file may not be a symbolic link: ${path}"
    if [[ -e "${path}" ]]; then
        [[ -f "${path}" ]] || die "managed file path is not a regular file: ${path}"
        if [[ "${allow_hardlinks}" != "true" ]]; then
            [[ "$(stat_file_link_count "${path}")" == "1" ]] \
                || die "managed file may not have multiple hard links: ${path}"
        fi
    fi
}

validate_managed_parent_directory() {
    local path="$1"
    local parent
    parent="$(dirname -- "${path}")"
    [[ -d "${parent}" && ! -L "${parent}" ]] \
        || die "managed file parent must be a real directory: ${parent}"
}

validate_privileged_path_ancestor() {
    local path="$1"
    local ancestor canonical current mode permissions first
    ancestor="${path}"
    if [[ ! -d "${ancestor}" ]]; then
        ancestor="$(dirname -- "${ancestor}")"
    fi
    while [[ ! -e "${ancestor}" && ! -L "${ancestor}" ]]; do
        [[ "${ancestor}" != "/" && "${ancestor}" != "." ]] \
            || die "could not resolve a trusted ancestor for privileged path: ${path}"
        ancestor="$(dirname -- "${ancestor}")"
    done
    [[ -d "${ancestor}" ]] \
        || die "privileged path ancestor is not a directory: ${ancestor}"

    current="${ancestor}"
    first="true"
    while :; do
        [[ "$(stat_file_uid "${current}")" == "0" ]] \
            || die "privileged path component must be owned by root: ${current}"
        if [[ ! -L "${current}" ]]; then
            [[ -d "${current}" ]] \
                || die "privileged path component is not a directory: ${current}"
            mode="$(stat_file_mode "${current}")"
            [[ "${mode}" =~ ^[0-7]+$ ]] \
                || die "privileged path component has an invalid mode: ${current}"
            permissions=$((0${mode}))
            if (( (permissions & 0022) != 0 )); then
                if [[ "${first}" == "true" ]] || (( (permissions & 1000) == 0 )); then
                    die "privileged path component may not be group- or world-writable: ${current}"
                fi
            fi
        fi
        [[ "${current}" != "/" ]] || break
        current="$(dirname -- "${current}")"
        first="false"
    done

    canonical="$(cd -- "${ancestor}" && pwd -P)" \
        || die "could not resolve privileged path ancestor: ${ancestor}"
    current="${canonical}"
    first="true"
    while :; do
        [[ -d "${current}" && ! -L "${current}" ]] \
            || die "resolved privileged path component is not a real directory: ${current}"
        [[ "$(stat_file_uid "${current}")" == "0" ]] \
            || die "resolved privileged path component must be owned by root: ${current}"
        mode="$(stat_file_mode "${current}")"
        [[ "${mode}" =~ ^[0-7]+$ ]] \
            || die "resolved privileged path component has an invalid mode: ${current}"
        permissions=$((0${mode}))
        if (( (permissions & 0022) != 0 )); then
            if [[ "${first}" == "true" ]] || (( (permissions & 1000) == 0 )); then
                die "resolved privileged path component may not be group- or world-writable: ${current}"
            fi
        fi
        [[ "${current}" != "/" ]] || break
        current="$(dirname -- "${current}")"
        first="false"
    done
}

atomic_install_managed_file() {
    local source="$1"
    local target="$2"
    local mode="$3"
    local owner="${4:-}"
    local group="${5:-}"
    local parent base temporary

    [[ -f "${source}" && ! -L "${source}" ]] \
        || die "managed file source must be a regular file: ${source}"
    if [[ "${EUID}" -eq 0 && ( -n "${owner}" || -n "${group}" ) ]]; then
        validate_privileged_path_ancestor "${target}"
    fi
    validate_managed_parent_directory "${target}"
    validate_managed_regular_file "${target}" true
    parent="$(dirname -- "${target}")"
    base="$(basename -- "${target}")"
    temporary="$(mktemp "${parent}/.${base}.tmp.XXXXXXXX")" \
        || die "could not create a temporary managed file in ${parent}"

    local -a install_args=(-m "${mode}")
    [[ -z "${owner}" ]] || install_args+=(-o "${owner}")
    [[ -z "${group}" ]] || install_args+=(-g "${group}")
    if ! install "${install_args[@]}" "${source}" "${temporary}"; then
        rm -f -- "${temporary}"
        die "could not stage managed file: ${target}"
    fi

    validate_managed_parent_directory "${target}"
    if [[ -L "${target}" || ( -e "${target}" && ! -f "${target}" ) ]]; then
        rm -f -- "${temporary}"
        die "managed file target changed to an unsafe type: ${target}"
    fi
    case "$(install_os)" in
        linux)
            if ! mv -fT -- "${temporary}" "${target}"; then
                rm -f -- "${temporary}"
                die "could not atomically replace managed file: ${target}"
            fi
            ;;
        macos)
            if ! mv -fh -- "${temporary}" "${target}"; then
                rm -f -- "${temporary}"
                die "could not atomically replace managed file: ${target}"
            fi
            ;;
    esac
    [[ -f "${target}" && ! -L "${target}" ]] \
        || die "managed file replacement did not produce a regular file: ${target}"
}

ensure_privileged_directory() {
    local path="$1"
    local mode="$2"
    local owner="$3"
    local group="$4"

    validate_privileged_path_ancestor "${path}"
    [[ ! -L "${path}" ]] || die "privileged directory may not be a symbolic link: ${path}"
    if [[ -e "${path}" && ! -d "${path}" ]]; then
        die "privileged directory path is not a directory: ${path}"
    fi
    install -d -o "${owner}" -g "${group}" -m "${mode}" "${path}"
    [[ -d "${path}" && ! -L "${path}" ]] \
        || die "privileged directory became an unsafe path: ${path}"
}

resolve_compose_dir() {
    if [[ -n "${COMPOSE_DIR}" ]]; then
        return
    fi

    if [[ "${INSTALL_ROOT_EXPLICIT}" == "true" || "${COMPOSE_DIR_EXPLICIT}" == "true" ]]; then
        COMPOSE_DIR="${INSTALL_ROOT}/compose"
    else
        COMPOSE_DIR="$(pwd -P)"
    fi
}

install_project_file() {
    local source_path="$1"
    local target_path="$2"
    local mode="$3"
    local script_dir
    script_dir="$(current_script_dir || true)"

    ensure_directory "$(dirname "${target_path}")"
    if [[ -n "${script_dir}" && -f "${script_dir}/${source_path}" && ! -L "${script_dir}/${source_path}" ]]; then
        atomic_install_managed_file \
            "${script_dir}/${source_path}" "${target_path}" "${mode}"
    else
        local downloaded
        downloaded="$(mktemp)"
        download_to "$(raw_project_url "${source_path}")" "${downloaded}"
        atomic_install_managed_file "${downloaded}" "${target_path}" "${mode}"
        rm -f -- "${downloaded}"
    fi
}

install_generate_keys_script() {
    local target_path="$1"
    local script_dir
    script_dir="$(current_script_dir || true)"

    ensure_directory "$(dirname "${target_path}")"
    if [[ -n "${script_dir}" && -f "${script_dir}/generate_keys.sh" && ! -L "${script_dir}/generate_keys.sh" ]]; then
        atomic_install_managed_file \
            "${script_dir}/generate_keys.sh" "${target_path}" 0755
    else
        write_generate_keys_script "${target_path}"
    fi
}

ensure_directory() {
    local path="$1"
    local mode="${2:-0755}"
    [[ ! -L "${path}" ]] || die "managed directory may not be a symbolic link: ${path}"
    if [[ -e "${path}" && ! -d "${path}" ]]; then
        die "managed directory path is not a directory: ${path}"
    fi
    if [[ ! -d "${path}" ]]; then
        install -d -m "${mode}" "${path}"
    fi
    [[ ! -L "${path}" ]] || die "managed directory became a symbolic link: ${path}"
}

require_compose_runtime() {
    resolve_compose_cli
}

resolve_compose_cli() {
    if [[ "${#COMPOSE_CLI[@]}" -gt 0 ]]; then
        return
    fi

    if docker compose version >/dev/null 2>&1; then
        COMPOSE_CLI=(docker compose)
        return
    fi

    if command -v docker-compose >/dev/null 2>&1; then
        COMPOSE_CLI=(docker-compose)
        return
    fi

    if ui_is_zh; then
        die "未找到可用的 Docker Compose，请先安装 Docker 和 Compose 插件"
    else
        die "no usable Docker Compose found; install Docker and the Compose plugin first"
    fi
}

compose_command() {
    resolve_compose_cli
    printf '%s\n' "${COMPOSE_CLI[*]}"
}

run_compose() {
    resolve_compose_cli
    "${COMPOSE_CLI[@]}" "$@"
}

compose_next_steps() {
    local gateway_port
    local compose_cmd
    compose_cmd="$(compose_command)"
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${COMPOSE_DIR}/.env" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"

    cat <<EOF

Install complete.

Docker Compose service:
  cd ${COMPOSE_DIR}
  ./update.sh
  ${compose_cmd} -f docker-compose.yml ps
  ${compose_cmd} -f docker-compose.yml logs -f app

Health checks:
  curl -fsS http://127.0.0.1:${gateway_port}/_gateway/health
  curl -fsS http://127.0.0.1:${gateway_port}/readyz

Install directory:
  ${COMPOSE_DIR}

EOF
}

compose_manual_start_steps() {
    local compose_cmd
    compose_cmd="$(compose_command)"

    cat <<EOF

Next steps:
  cd ${COMPOSE_DIR}
  ${compose_cmd} -f docker-compose.yml pull
  ${compose_cmd} -f docker-compose.yml up -d
  ${compose_cmd} -f docker-compose.yml logs -f app

Later updates:
  cd ${COMPOSE_DIR}
  ./update.sh

Generate a fresh key set any time:
  cd ${COMPOSE_DIR}
  ./generate_keys.sh
EOF
}

start_compose_deployment() {
    local -a compose_args=(--project-directory "${COMPOSE_DIR}" -f "${COMPOSE_DIR}/docker-compose.yml")

    info "pulling Docker Compose images"
    run_compose "${compose_args[@]}" pull
    info "starting Docker Compose services"
    run_compose "${compose_args[@]}" up -d
}

resolve_version() {
    if [[ -n "${VERSION}" ]]; then
        echo "${VERSION}"
        return
    fi

    local tag=""
    case "${CHANNEL}" in
        stable|latest)
            tag="$(download_stdout "https://api.github.com/repos/${REPO}/releases?per_page=50" |
                sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
                grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' |
                head -n1 || true)"
            ;;
        rc)
            tag="$(download_stdout "https://api.github.com/repos/${REPO}/releases?per_page=50" |
                sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
                grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$' |
                head -n1 || true)"
            ;;
        beta)
            tag="$(download_stdout "https://api.github.com/repos/${REPO}/releases?per_page=50" |
                sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
                grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$' |
                head -n1 || true)"
            ;;
        nightly)
            # The nightly release is a single rolling tag, so no API listing is
            # needed (and unauthenticated release-list calls are rate-limited).
            tag="nightly"
            ;;
        *)
            die "unsupported release channel: ${CHANNEL}; expected stable, latest, rc, beta, or nightly"
            ;;
    esac
    echo "${tag}"
}

resolve_compose_release_identity() {
    local tag
    tag="$(resolve_version)"
    [[ -n "${tag}" ]] || die "could not resolve ${CHANNEL} release tag for ${REPO}"
    validate_release_identifier "${tag}"
    VERSION="${tag}"
    if [[ "${SOURCE_REF_EXPLICIT}" != "true" ]]; then
        SOURCE_REF="${tag}"
    fi
    validate_installer_source_identifiers
}

current_script_dir() {
    local source="${BASH_SOURCE[0]:-}"
    [[ -n "${source}" && -f "${source}" ]] || return 1

    while [[ -L "${source}" ]]; do
        local source_dir target
        source_dir="$(cd -- "$(dirname -- "${source}")" && pwd -P)"
        target="$(readlink "${source}")"
        if [[ "${target}" == /* ]]; then
            source="${target}"
        else
            source="${source_dir}/${target}"
        fi
    done
    [[ -f "${source}" ]] || return 1
    cd -- "$(dirname -- "${source}")" && pwd -P
}

ensure_tmp_root() {
    if [[ -z "${TMP_ROOT}" ]]; then
        TMP_ROOT="$(mktemp -d)"
    fi
}

absolute_path() {
    local path="$1"
    local dir
    local base

    if [[ "${path}" == /* ]]; then
        printf '%s\n' "${path}"
        return
    fi

    dir="$(dirname "${path}")"
    base="$(basename "${path}")"
    printf '%s/%s\n' "$(cd "${dir}" && pwd -P)" "${base}"
}

absolute_path_maybe_missing() {
    local path="$1"
    if [[ "${path}" == /* ]]; then
        printf '%s\n' "${path}"
    else
        printf '%s/%s\n' "$(pwd -P)" "${path}"
    fi
}

local_bundle_dir() {
    local dir
    dir="$(current_script_dir || true)"
    [[ -n "${dir}" ]] || return 1
    if [[ -d "${dir}" && ! -L "${dir}" \
        && -d "${dir}/bin" && ! -L "${dir}/bin" \
        && -f "${dir}/bin/aether-gateway" \
        && ! -L "${dir}/bin/aether-gateway" \
        && -x "${dir}/bin/aether-gateway" \
        && -d "${dir}/frontend" && ! -L "${dir}/frontend" ]]; then
        echo "${dir}"
    fi
}

validate_local_bundle_tree() {
    local bundle="$1"
    local binary="${bundle}/bin/aether-gateway"
    local unsafe_path

    [[ -d "${bundle}" && ! -L "${bundle}" ]] \
        || die "release bundle must be a real directory: ${bundle}"
    [[ -d "${bundle}/bin" && ! -L "${bundle}/bin" ]] \
        || die "release bundle bin path must be a real directory: ${bundle}/bin"
    [[ -f "${binary}" && ! -L "${binary}" && -x "${binary}" ]] \
        || die "release bundle binary must be a regular executable file: ${binary}"
    [[ "$(stat_file_link_count "${binary}")" == "1" ]] \
        || die "release bundle binary may not have multiple hard links: ${binary}"
    [[ -d "${bundle}/frontend" && ! -L "${bundle}/frontend" ]] \
        || die "release bundle frontend path must be a real directory: ${bundle}/frontend"

    if ! unsafe_path="$(find "${bundle}" -type l -print -quit)"; then
        die "could not inspect release bundle for symbolic links: ${bundle}"
    fi
    [[ -z "${unsafe_path}" ]] \
        || die "release bundle may not contain symbolic links: ${unsafe_path}"

    if ! unsafe_path="$(find "${bundle}" ! -type d ! -type f -print -quit)"; then
        die "could not inspect release bundle entry types: ${bundle}"
    fi
    [[ -z "${unsafe_path}" ]] \
        || die "release bundle may contain only directories and regular files: ${unsafe_path}"

    if ! unsafe_path="$(find "${bundle}" -type f -links +1 -print -quit)"; then
        die "could not inspect release bundle hard links: ${bundle}"
    fi
    [[ -z "${unsafe_path}" ]] \
        || die "release bundle may not contain multiply-linked files: ${unsafe_path}"
}

download_or_unpack_bundle() {
    TMP_ROOT="$(mktemp -d)"
    local archive_file archive_root archive_source expected_root
    if [[ -n "${ARCHIVE_PATH}" ]]; then
        [[ -f "${ARCHIVE_PATH}" ]] || die "archive not found: ${ARCHIVE_PATH}"
        info "using local archive ${ARCHIVE_PATH}"
        archive_source="$(absolute_path "${ARCHIVE_PATH}")"
        archive_file="${TMP_ROOT}/local-release.tar.gz"
        cp "${archive_source}" "${archive_file}" \
            || die "could not copy local release archive into the validation directory"
        expected_root=""
    else
        local os arch
        os="$(install_os)"
        arch="$(detect_arch)"

        local tag asset base_url archive_url archive_file
        tag="$(resolve_version)"
        [[ -n "${tag}" ]] || die "could not resolve ${CHANNEL} release tag for ${REPO}"
        validate_release_identifier "${tag}"
        VERSION="${tag}"
        asset="aether-${tag}-${os}-${arch}.tar.gz"
        base_url="https://github.com/${REPO}/releases/download/${tag}"
        archive_url="${base_url}/${asset}"
        archive_file="${TMP_ROOT}/${asset}"

        select_release_download_urls "${archive_url}"
        if [[ "${RELEASE_ARCHIVE_URL}" == "${archive_url}" ]]; then
            info "downloading ${asset} from ${REPO}"
        elif ui_is_zh; then
            info "从自定义 URL 下载 ${asset}"
        else
            info "downloading ${asset} from custom URL"
        fi
        download_to "${RELEASE_ARCHIVE_URL}" "${archive_file}" progress
        download_to "${base_url}/SHA256SUMS" "${TMP_ROOT}/SHA256SUMS"
        verify_release_checksum "${archive_file}" "${TMP_ROOT}/SHA256SUMS" "${asset}"
        expected_root="${asset%.tar.gz}"
    fi

    archive_root="$(validate_release_archive "${archive_file}" "${expected_root}")"
    extract_validated_release_archive "${archive_file}"
    local bundle="${TMP_ROOT}/${archive_root}"
    [[ -n "${bundle}" ]] || die "release archive did not contain a bundle directory"
    validate_local_bundle_tree "${bundle}"
    if [[ -z "${VERSION}" ]]; then
        VERSION="$(derive_local_bundle_version "${bundle}")"
    fi
    validate_release_identifier "${VERSION}"
    BUNDLE_DIR="${bundle}"
}

urlsafe_rand() {
    local bytes="$1"
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -base64 "${bytes}" | tr '+/' '-_' | tr -d '='
    else
        od -An -N "${bytes}" -tx1 /dev/urandom | tr -d ' \n'
    fi
}

write_generate_keys_script() {
    local output="$1"
    local output_dir output_dir_normalized config_dir_normalized rendered
    output_dir="$(dirname "${output}")"
    output_dir_normalized="${output_dir%/}"
    config_dir_normalized="${CONFIG_DIR%/}"
    [[ -n "${output_dir_normalized}" ]] || output_dir_normalized="/"
    [[ -n "${config_dir_normalized}" ]] || config_dir_normalized="/"
    if is_darwin && [[ "${output_dir_normalized}" == "${config_dir_normalized}" ]]; then
        install_config_dir
    else
        ensure_directory "${output_dir}"
    fi
    rendered="$(mktemp)"
    cat > "${rendered}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

urlsafe_rand() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -base64 "$1" | tr '+/' '-_' | tr -d '='
    else
        od -An -N "$1" -tx1 /dev/urandom | tr -d ' \n'
    fi
}

cat <<KEYS
JWT_SECRET_KEY=$(urlsafe_rand 32)
ENCRYPTION_KEY=$(urlsafe_rand 32)
DB_PASSWORD=$(urlsafe_rand 32)
REDIS_PASSWORD=$(urlsafe_rand 32)
KEYS
EOF
    atomic_install_managed_file "${rendered}" "${output}" 0755
    rm -f -- "${rendered}"
}

validate_dotenv_scalar() {
    local key="$1"
    local value="$2"

    [[ "${value}" != *$'\n'* && "${value}" != *$'\r'* ]] \
        || die "${key} may not contain CR or LF characters"
}

replace_or_append_env() {
    local file="$1"
    local key="$2"
    local value="$3"
    local parent base staged mode owner group line
    local replaced="false"

    [[ "${key}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] \
        || die "invalid dotenv key: ${key}"
    validate_dotenv_scalar "${key}" "${value}"
    validate_managed_parent_directory "${file}"
    validate_managed_regular_file "${file}" true
    parent="$(dirname -- "${file}")"
    base="$(basename -- "${file}")"
    staged="$(mktemp "${parent}/.${base}.edit.XXXXXXXX")" \
        || die "could not create a temporary env file in ${parent}"

    if [[ -e "${file}" ]]; then
        while IFS= read -r line || [[ -n "${line}" ]]; do
            if [[ "${replaced}" == "false" && "${line}" =~ ^#?[[:space:]]*${key}= ]]; then
                printf '%s=%s\n' "${key}" "${value}" >>"${staged}"
                replaced="true"
            else
                printf '%s\n' "${line}" >>"${staged}"
            fi
        done <"${file}"
    fi
    if [[ "${replaced}" == "false" ]]; then
        printf '%s=%s\n' "${key}" "${value}" >> "${staged}"
    fi

    mode="0600"
    owner=""
    group=""
    if [[ -e "${file}" ]]; then
        mode="$(stat_file_mode "${file}")"
        if [[ "${EUID}" -eq 0 ]]; then
            owner="$(stat_file_uid "${file}")"
            group="$(stat_file_gid "${file}")"
        fi
    fi
    atomic_install_managed_file "${staged}" "${file}" "${mode}" "${owner}" "${group}"
    rm -f -- "${staged}"
}

trim_whitespace() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "${value}"
}

strip_optional_quotes() {
    local value="$1"
    if [[ ${#value} -ge 2 ]]; then
        if [[ "${value:0:1}" == "\"" && "${value: -1}" == "\"" ]]; then
            value="${value:1:${#value}-2}"
        elif [[ "${value:0:1}" == "'" && "${value: -1}" == "'" ]]; then
            value="${value:1:${#value}-2}"
        fi
    fi
    printf '%s' "${value}"
}

is_placeholder_value() {
    local value="$1"
    case "${value}" in
        *change-me*|*change-this*|*your_secure_password_here*|*your_redis_password_here*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

derive_local_bundle_version() {
    local bundle="$1"
    local name
    name="$(basename "${bundle}")"
    case "${name}" in
        aether-*-linux-*|aether-*-macos-*)
            name="${name#aether-}"
            name="${name%-linux-*}"
            name="${name%-macos-*}"
            ;;
    esac
    if [[ -z "${name}" || "${name}" == "." || "${name}" == "/" ]]; then
        name="$(date +%Y%m%d%H%M%S)"
    fi
    echo "${name}"
}

generate_first_install_env() {
    local output="$1"
    local database_url="${AETHER_DATABASE_URL:-${DATABASE_URL:-${AETHER_GATEWAY_DATA_POSTGRES_URL:-}}}"
    [[ -n "${database_url}" ]] || die "PostgreSQL DATABASE_URL is required for system-service installs; set DATABASE_URL or use --mode compose"
    case "${database_url}" in
        postgres://*|postgresql://*) ;;
        *) die "only PostgreSQL database URLs are supported" ;;
    esac
    validate_dotenv_scalar "DATABASE_URL" "${database_url}"
    local jwt_key encryption_key
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"
    validate_dotenv_scalar "ADMIN_PASSWORD" "${ADMIN_PASSWORD}"

    cat > "${output}" <<EOF
ENVIRONMENT=production
TZ=Asia/Shanghai
RUST_LOG=aether_gateway=info
AETHER_LOG_DESTINATION=both
AETHER_LOG_FORMAT=pretty
AETHER_LOG_DIR=${INSTALL_ROOT}/logs
AETHER_LOG_ROTATION=daily
AETHER_LOG_RETENTION_DAYS=7
AETHER_LOG_MAX_FILES=30

APP_PORT=${APP_PORT:-8084}
AETHER_BASE_DIR=${INSTALL_ROOT}
AETHER_UPDATE_STRATEGY=self
AETHER_GATEWAY_STATIC_DIR=${INSTALL_ROOT}/current/frontend
AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=rust-authoritative
AETHER_GATEWAY_DATABASE_MODE=auto
AETHER_RUNTIME_BACKEND=memory
API_KEY_PREFIX=sk

AETHER_DATABASE_DRIVER=postgres
AETHER_DATABASE_URL=${database_url}
DATABASE_URL=${database_url}

JWT_SECRET_KEY=${jwt_key}
ENCRYPTION_KEY=${encryption_key}

ADMIN_EMAIL=admin@example.local
ADMIN_USERNAME=admin
ADMIN_PASSWORD=${ADMIN_PASSWORD}
EOF
}

generate_cluster_env() {
    local output="$1"
    local jwt_key encryption_key role
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"
    role="${AETHER_GATEWAY_NODE_ROLE:-frontdoor}"
    validate_dotenv_scalar "ADMIN_PASSWORD" "${ADMIN_PASSWORD}"
    validate_dotenv_scalar "ADMIN_EMAIL" "${ADMIN_EMAIL:-admin@example.local}"
    validate_dotenv_scalar "ADMIN_USERNAME" "${ADMIN_USERNAME:-admin}"
    validate_dotenv_scalar "DATABASE_URL" "${DATABASE_URL:-}"
    validate_dotenv_scalar "REDIS_URL" "${REDIS_URL:-}"

    cat > "${output}" <<EOF
ENVIRONMENT=production
TZ=Asia/Shanghai
RUST_LOG=aether_gateway=info
AETHER_LOG_DESTINATION=both
AETHER_LOG_FORMAT=pretty
AETHER_LOG_DIR=${INSTALL_ROOT}/logs
AETHER_LOG_ROTATION=daily
AETHER_LOG_RETENTION_DAYS=7
AETHER_LOG_MAX_FILES=30

APP_PORT=${APP_PORT:-8084}
AETHER_BASE_DIR=${INSTALL_ROOT}
AETHER_UPDATE_STRATEGY=manual
AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=multi-node
AETHER_GATEWAY_NODE_ROLE=${role}
AETHER_GATEWAY_STATIC_DIR=${INSTALL_ROOT}/current/frontend
AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=rust-authoritative
AETHER_GATEWAY_DATABASE_MODE=auto
AETHER_RUNTIME_BACKEND=redis
API_KEY_PREFIX=sk

DATABASE_URL=${DATABASE_URL:-}
REDIS_URL=${REDIS_URL:-}

JWT_SECRET_KEY=${jwt_key}
ENCRYPTION_KEY=${encryption_key}

ADMIN_EMAIL=${ADMIN_EMAIL:-admin@example.local}
ADMIN_USERNAME=${ADMIN_USERNAME:-admin}
ADMIN_PASSWORD=${ADMIN_PASSWORD}
EOF
}

compose_image() {
    if [[ -n "${APP_IMAGE}" ]]; then
        echo "${APP_IMAGE}"
        return
    fi

    local tag=""
    if [[ -n "${VERSION}" ]]; then
        tag="${VERSION#v}"
    else
        case "${CHANNEL}" in
            stable|latest)
                tag="latest"
                ;;
            rc|beta|nightly)
                tag="${CHANNEL}"
                ;;
            *)
                die "unsupported release channel: ${CHANNEL}; expected stable, latest, rc, beta, or nightly"
                ;;
        esac
    fi

    printf '%s:%s\n' "${IMAGE_REPO}" "${tag}"
}

compose_app_port() {
    printf '%s\n' "${APP_PORT:-${COMPOSE_APP_PORT_DEFAULT}}"
}

append_compose_log_env_defaults() {
    local output="$1"
    replace_or_append_env "${output}" "AETHER_LOG_DESTINATION" "${COMPOSE_LOG_DESTINATION_DEFAULT}"
    replace_or_append_env "${output}" "AETHER_LOG_FORMAT" "${COMPOSE_LOG_FORMAT_DEFAULT}"
    replace_or_append_env "${output}" "AETHER_LOG_DIR" "${COMPOSE_RELEASE_LOG_DIR}"
    replace_or_append_env "${output}" "AETHER_LOG_ROTATION" "${COMPOSE_LOG_ROTATION_DEFAULT}"
    replace_or_append_env "${output}" "AETHER_LOG_RETENTION_DAYS" "${COMPOSE_LOG_RETENTION_DAYS_DEFAULT}"
    replace_or_append_env "${output}" "AETHER_LOG_MAX_FILES" "${COMPOSE_LOG_MAX_FILES_DEFAULT}"
}

generate_compose_env() {
    local output="$1"
    local jwt_key encryption_key db_password redis_password
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"
    db_password="$(urlsafe_rand 32)"
    redis_password="$(urlsafe_rand 32)"

    cp "${COMPOSE_DIR}/.env.example" "${output}"
    replace_or_append_env "${output}" "APP_IMAGE" "$(compose_image)"
    replace_or_append_env "${output}" "APP_PORT" "$(compose_app_port)"
    replace_or_append_env "${output}" "DB_PASSWORD" "${db_password}"
    replace_or_append_env "${output}" "REDIS_PASSWORD" "${redis_password}"
    replace_or_append_env "${output}" "JWT_SECRET_KEY" "${JWT_SECRET_KEY:-${jwt_key}}"
    replace_or_append_env "${output}" "ENCRYPTION_KEY" "${ENCRYPTION_KEY:-${encryption_key}}"
    replace_or_append_env "${output}" "ADMIN_EMAIL" "${ADMIN_EMAIL:-admin@example.local}"
    replace_or_append_env "${output}" "ADMIN_USERNAME" "${ADMIN_USERNAME:-admin}"
    replace_or_append_env "${output}" "ADMIN_PASSWORD" "${ADMIN_PASSWORD}"
    replace_or_append_env "${output}" "AETHER_UPDATE_STRATEGY" "docker"
    replace_or_append_env "${output}" "AETHER_DOCKER_UPDATE_COMMAND" "./update.sh"
    append_compose_log_env_defaults "${output}"
    replace_or_append_env "${output}" "AETHER_GATEWAY_DATABASE_MODE" "auto"
}

generate_compose_single_node_env() {
    local output="$1"
    generate_compose_env "${output}"
    replace_or_append_env "${output}" "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY" "single-node"
    replace_or_append_env "${output}" "AETHER_DATABASE_DRIVER" "postgres"
}

install_config_dir() {
    validate_privileged_path_ancestor "${CONFIG_DIR}"
    [[ ! -L "${CONFIG_DIR}" ]] || die "config directory may not be a symbolic link: ${CONFIG_DIR}"
    if [[ -e "${CONFIG_DIR}" && ! -d "${CONFIG_DIR}" ]]; then
        die "config directory path is not a directory: ${CONFIG_DIR}"
    fi
    if is_darwin; then
        install -d -o root -g "${SERVICE_GROUP}" -m 0750 "${CONFIG_DIR}"
    else
        install -d -o root -g root -m 0750 "${CONFIG_DIR}"
    fi
    [[ ! -L "${CONFIG_DIR}" ]] || die "config directory became a symbolic link: ${CONFIG_DIR}"
}

install_env_target_from() {
    local source="$1"
    local owner="" group=""
    [[ -f "${source}" && ! -L "${source}" ]] \
        || die "env source must be a regular file and not a symbolic link: ${source}"
    if is_darwin; then
        if [[ "${EUID}" -eq 0 ]]; then
            owner="root"
            group="${SERVICE_GROUP}"
        fi
        atomic_install_managed_file "${source}" "${ENV_TARGET}" 0640 "${owner}" "${group}"
    else
        if [[ "${EUID}" -eq 0 ]]; then
            owner="root"
            group="root"
        fi
        atomic_install_managed_file "${source}" "${ENV_TARGET}" 0600 "${owner}" "${group}"
    fi
}

ensure_env_target_permissions() {
    local owner="" group="" mode="0600"
    validate_managed_regular_file "${ENV_TARGET}" true
    [[ -f "${ENV_TARGET}" ]] || die "env target is missing: ${ENV_TARGET}"
    if [[ "${EUID}" -eq 0 ]]; then
        owner="root"
        group="root"
    fi
    if is_darwin; then
        mode="0640"
        if [[ "${EUID}" -eq 0 ]]; then
            group="${SERVICE_GROUP}"
        fi
    fi
    atomic_install_managed_file \
        "${ENV_TARGET}" "${ENV_TARGET}" "${mode}" "${owner}" "${group}"
}

install_systemd_support_files() {
    install_config_dir
    write_generate_keys_script "${CONFIG_DIR}/generate_keys.sh"
}

find_nologin_shell() {
    if [[ -x /usr/sbin/nologin ]]; then
        echo "/usr/sbin/nologin"
    elif [[ -x /sbin/nologin ]]; then
        echo "/sbin/nologin"
    else
        echo "/bin/false"
    fi
}

ensure_service_account() {
    if ! getent group "${SERVICE_GROUP}" >/dev/null 2>&1; then
        info "creating group ${SERVICE_GROUP}"
        groupadd --system "${SERVICE_GROUP}"
    fi

    if ! id -u "${SERVICE_USER}" >/dev/null 2>&1; then
        info "creating user ${SERVICE_USER}"
        useradd \
            --system \
            --gid "${SERVICE_GROUP}" \
            --home-dir "${INSTALL_ROOT}" \
            --shell "$(find_nologin_shell)" \
            "${SERVICE_USER}"
    fi
}

macos_next_system_id() {
    local record_type="$1"
    local id_attr="$2"
    dscl . -list "/${record_type}" "${id_attr}" 2>/dev/null |
        awk '
            $NF ~ /^[0-9]+$/ && $NF >= 350 && $NF < 500 { used[$NF] = 1 }
            END {
                for (i = 350; i < 500; i++) {
                    if (!(i in used)) {
                        print i
                        exit
                    }
                }
            }
        '
}

macos_group_id() {
    dscl . -read "/Groups/${SERVICE_GROUP}" PrimaryGroupID 2>/dev/null |
        awk '/PrimaryGroupID:/ { print $2 }'
}

ensure_macos_service_account() {
    local gid uid

    if ! command -v dscl >/dev/null 2>&1; then
        if ui_is_zh; then
            die "未找到 dscl，无法创建 macOS 服务账号"
        else
            die "dscl not found; cannot create macOS service account"
        fi
    fi

    if ! dscl . -read "/Groups/${SERVICE_GROUP}" >/dev/null 2>&1; then
        gid="$(macos_next_system_id Groups PrimaryGroupID)"
        [[ -n "${gid}" ]] || die "could not allocate a macOS service group id"
        info "creating macOS group ${SERVICE_GROUP}"
        dscl . -create "/Groups/${SERVICE_GROUP}"
        dscl . -create "/Groups/${SERVICE_GROUP}" PrimaryGroupID "${gid}"
        dscl . -create "/Groups/${SERVICE_GROUP}" Password "*"
    fi

    gid="$(macos_group_id)"
    [[ -n "${gid}" ]] || die "could not resolve macOS group id for ${SERVICE_GROUP}"

    if ! dscl . -read "/Users/${SERVICE_USER}" >/dev/null 2>&1; then
        uid="$(macos_next_system_id Users UniqueID)"
        [[ -n "${uid}" ]] || die "could not allocate a macOS service user id"
        info "creating macOS user ${SERVICE_USER}"
        dscl . -create "/Users/${SERVICE_USER}"
        dscl . -create "/Users/${SERVICE_USER}" UserShell /usr/bin/false
        dscl . -create "/Users/${SERVICE_USER}" RealName "Aether Gateway"
        dscl . -create "/Users/${SERVICE_USER}" UniqueID "${uid}"
        dscl . -create "/Users/${SERVICE_USER}" PrimaryGroupID "${gid}"
        dscl . -create "/Users/${SERVICE_USER}" NFSHomeDirectory "${INSTALL_ROOT}"
        dscl . -create "/Users/${SERVICE_USER}" IsHidden 1
        dscl . -create "/Users/${SERVICE_USER}" Password "*"
    fi
}

env_file_value() {
    local file="$1"
    local key="$2"
    awk -v key="${key}" '
        {
            line = $0
            sub(/^[[:space:]]*/, "", line)
            if (line ~ /^#/ || line !~ /^[A-Za-z_][A-Za-z0-9_]*=/) {
                next
            }
            name = line
            sub(/=.*/, "", name)
            if (name == key) {
                value = line
                sub(/^[^=]*=/, "", value)
                print value
            }
        }
    ' "${file}" | tail -n1 | tr -d '[:space:]'
}

ensure_env_matches_requested_mode() {
    local file="$1"
    local mode="$2"
    local topology
    topology="$(env_file_value "${file}" "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY")"
    topology="${topology:-single-node}"

    if [[ "${mode}" == "cluster" ]]; then
        [[ "${topology}" == "multi-node" ]] || die "existing env ${file} is ${topology}; set AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=multi-node or use --mode single-node"
        cluster_env_has_required_backends "${file}" || die "existing multi-node env ${file} must define DATABASE_URL and REDIS_URL"
    elif [[ "${mode}" == "single-node" && "${topology}" == "multi-node" ]]; then
        die "existing env ${file} is multi-node; cluster mode is temporarily disabled, edit the env file"
    fi
}

cluster_env_has_required_backends() {
    local file="$1"
    local database_url redis_url
    database_url="$(env_file_value "${file}" "AETHER_DATABASE_URL")"
    [[ -n "${database_url}" ]] || database_url="$(env_file_value "${file}" "DATABASE_URL")"
    [[ -n "${database_url}" ]] || database_url="$(env_file_value "${file}" "AETHER_GATEWAY_DATA_POSTGRES_URL")"
    redis_url="$(env_file_value "${file}" "REDIS_URL")"
    [[ -n "${redis_url}" ]] || redis_url="$(env_file_value "${file}" "AETHER_GATEWAY_DATA_REDIS_URL")"

    [[ -n "${database_url}" && -n "${redis_url}" ]]
}

validate_env_file() {
    local env_file="$1"
    local raw_line=""
    local line=""
    local key=""
    local value=""
    local line_no=0
    local topology="single-node"
    local node_role="all"
    local database_driver=""
    local runtime_backend=""
    local db_password=""
    local redis_password=""
    local database_url=""
    local redis_url=""
    local jwt_secret_key=""
    local encryption_key=""
    local video_task_store_path=""
    local static_dir=""

    [[ -f "${env_file}" ]] || die "env file not found: ${env_file}"

    info "validating env file ${env_file}"
    while IFS= read -r raw_line || [[ -n "${raw_line}" ]]; do
        line_no=$((line_no + 1))
        line="${raw_line%$'\r'}"
        line="$(trim_whitespace "${line}")"

        [[ -z "${line}" ]] && continue
        [[ "${line:0:1}" == "#" ]] && continue

        [[ "${line}" == export\ * ]] && die "env file ${env_file}:${line_no} must not use 'export'"
        [[ "${line}" == *'${'* ]] && die "env file ${env_file}:${line_no} must not use variable expansion"
        [[ "${line}" == *'$('* ]] && die "env file ${env_file}:${line_no} must not use command substitution"
        [[ "${line}" == *'`'* ]] && die "env file ${env_file}:${line_no} must not use command substitution"
        [[ "${line}" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]] || die "env file ${env_file}:${line_no} must be KEY=VALUE"

        key="${line%%=*}"
        value="${line#*=}"
        value="$(strip_optional_quotes "${value}")"

        case "${key}" in
            AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY)
                topology="${value}"
                ;;
            AETHER_GATEWAY_NODE_ROLE)
                node_role="${value}"
                ;;
            AETHER_DATABASE_DRIVER)
                database_driver="$(printf '%s' "${value}" | tr '[:upper:]' '[:lower:]')"
                ;;
            AETHER_RUNTIME_BACKEND)
                runtime_backend="$(printf '%s' "${value}" | tr '[:upper:]' '[:lower:]')"
                ;;
            AETHER_DATABASE_URL|DATABASE_URL|AETHER_GATEWAY_DATA_POSTGRES_URL)
                [[ -n "${value}" ]] && database_url="${value}"
                ;;
            REDIS_URL|AETHER_GATEWAY_DATA_REDIS_URL)
                [[ -n "${value}" ]] && redis_url="${value}"
                ;;
            DB_PASSWORD)
                db_password="${value}"
                ;;
            REDIS_PASSWORD)
                redis_password="${value}"
                ;;
            JWT_SECRET_KEY)
                jwt_secret_key="${value}"
                ;;
            ENCRYPTION_KEY|AETHER_GATEWAY_DATA_ENCRYPTION_KEY)
                [[ -n "${value}" ]] && encryption_key="${value}"
                ;;
            AETHER_GATEWAY_VIDEO_TASK_STORE_PATH)
                video_task_store_path="${value}"
                ;;
            AETHER_GATEWAY_STATIC_DIR)
                static_dir="${value}"
                ;;
        esac
    done < "${env_file}"

    case "${topology}" in
        single-node|multi-node)
            ;;
        *)
            die "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY must be single-node or multi-node"
            ;;
    esac

    case "${node_role}" in
        all|frontdoor|background)
            ;;
        *)
            die "AETHER_GATEWAY_NODE_ROLE must be all, frontdoor, or background"
            ;;
    esac

    [[ -n "${jwt_secret_key}" ]] || die "JWT_SECRET_KEY is required"
    [[ -n "${encryption_key}" ]] || die "ENCRYPTION_KEY or AETHER_GATEWAY_DATA_ENCRYPTION_KEY is required"

    is_placeholder_value "${jwt_secret_key}" && die "JWT_SECRET_KEY still uses the example placeholder"
    is_placeholder_value "${encryption_key}" && die "ENCRYPTION_KEY still uses the example placeholder"
    if [[ -n "${database_url}" ]] && is_placeholder_value "${database_url}"; then
        die "DATABASE_URL still uses the example placeholder"
    fi
    if [[ -n "${redis_url}" ]] && is_placeholder_value "${redis_url}"; then
        die "REDIS_URL still uses the example placeholder"
    fi

    case "${database_driver}" in
        ""|postgres|postgresql) ;;
        *) die "only PostgreSQL database drivers are supported" ;;
    esac
    case "${database_url}" in
        postgres://*|postgresql://*) ;;
        *) die "a PostgreSQL DATABASE_URL is required" ;;
    esac

    if [[ "${topology}" == "multi-node" ]]; then
        [[ "${node_role}" != "all" ]] || die "multi-node deployment requires AETHER_GATEWAY_NODE_ROLE=frontdoor or background"
        [[ -n "${database_url}" ]] || die "multi-node deployment requires AETHER_DATABASE_URL, DATABASE_URL, or AETHER_GATEWAY_DATA_POSTGRES_URL"
        [[ -n "${redis_url}" ]] || die "multi-node deployment requires REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL"
        [[ "${runtime_backend}" != "memory" ]] || die "multi-node deployment must not use AETHER_RUNTIME_BACKEND=memory"
        [[ -z "${video_task_store_path}" ]] || die "multi-node deployment must not set AETHER_GATEWAY_VIDEO_TASK_STORE_PATH"
    else
        if [[ "${node_role}" != "all" ]]; then
            warn "single-node deployment usually uses AETHER_GATEWAY_NODE_ROLE=all; split roles are not enabled by this installer"
        fi
        if [[ "${runtime_backend}" == "redis" && -z "${redis_url}" ]]; then
            die "AETHER_RUNTIME_BACKEND=redis requires REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL"
        fi
    fi

    if is_placeholder_value "${db_password}"; then
        warn "DB_PASSWORD still uses the example placeholder"
    fi
    if is_placeholder_value "${redis_password}"; then
        warn "REDIS_PASSWORD still uses the example placeholder"
    fi

    if [[ -n "${static_dir}" && "${static_dir}" != "${INSTALL_ROOT}/current/frontend" ]]; then
        warn "AETHER_GATEWAY_STATIC_DIR points to ${static_dir}; install script still publishes frontend to ${INSTALL_ROOT}/current/frontend"
    fi
}

resolve_service_env_source() {
    local mode="$1"
    if [[ -n "${ENV_SOURCE}" ]]; then
        [[ -f "${ENV_SOURCE}" ]] || die "env file not found: ${ENV_SOURCE}"
        ensure_env_matches_requested_mode "${ENV_SOURCE}" "${mode}"
        echo "${ENV_SOURCE}"
        return
    fi

    if [[ -f "${ENV_TARGET}" ]]; then
        ensure_env_matches_requested_mode "${ENV_TARGET}" "${mode}"
        echo ""
        return
    fi

    GENERATED_ENV="${TMP_ROOT:-$(mktemp -d)}/aether-gateway.env"
    if [[ -z "${TMP_ROOT}" ]]; then
        TMP_ROOT="$(dirname "${GENERATED_ENV}")"
    fi

    if [[ "${mode}" == "cluster" ]]; then
        info "generating multi-node env file"
        generate_cluster_env "${GENERATED_ENV}"
        if ! cluster_env_has_required_backends "${GENERATED_ENV}"; then
            install_config_dir
            install_env_target_from "${GENERATED_ENV}"
            cat <<EOF

Multi-node env scaffolded:
  ${ENV_TARGET}

Fill DATABASE_URL and REDIS_URL, then rerun:
  sudo AETHER_INSTALL_MODE=cluster bash install.sh

Or provide them non-interactively:
  curl -fsSL https://raw.githubusercontent.com/${REPO}/${SOURCE_REF}/install.sh | sudo DATABASE_URL=postgresql://... REDIS_URL=redis://... bash -s -- --mode cluster
EOF
            exit 1
        fi
    else
        info "generating first-install single-node env file"
        generate_first_install_env "${GENERATED_ENV}"
    fi
    echo "${GENERATED_ENV}"
}

install_compose_mode() {
    resolve_compose_dir
    info "preparing Docker Compose deployment in ${COMPOSE_DIR}"
    ensure_directory "${COMPOSE_DIR}"
    ensure_directory "${COMPOSE_DIR}/logs"
    install_project_file "docker-compose.yml" "${COMPOSE_DIR}/docker-compose.yml" "0644"
    install_project_file ".env.example" "${COMPOSE_DIR}/.env.example" "0644"
    install_project_file "update.sh" "${COMPOSE_DIR}/update.sh" "0755"
    install_generate_keys_script "${COMPOSE_DIR}/generate_keys.sh"

    validate_managed_regular_file "${COMPOSE_DIR}/.env" false
    if [[ -f "${COMPOSE_DIR}/.env" ]]; then
        warn "keeping existing ${COMPOSE_DIR}/.env"
    else
        local generated_compose_env
        info "generating ${COMPOSE_DIR}/.env"
        generated_compose_env="$(mktemp)"
        generate_compose_env "${generated_compose_env}"
        atomic_install_managed_file \
            "${generated_compose_env}" "${COMPOSE_DIR}/.env" 0600
        rm -f -- "${generated_compose_env}"
    fi

    cat <<EOF

Docker Compose files are ready:
  ${COMPOSE_DIR}/docker-compose.yml
  ${COMPOSE_DIR}/.env
  ${COMPOSE_DIR}/.env.example
  ${COMPOSE_DIR}/update.sh
  ${COMPOSE_DIR}/generate_keys.sh
  ${COMPOSE_DIR}/logs
EOF

    if [[ "${SKIP_START}" == "true" ]]; then
        compose_manual_start_steps
        return
    fi

    require_compose_runtime
    start_compose_deployment
    compose_next_steps
}

install_compose_single_node_mode() {
    resolve_compose_dir
    info "preparing Docker Compose single-node deployment in ${COMPOSE_DIR}"
    ensure_directory "${COMPOSE_DIR}"
    ensure_directory "${COMPOSE_DIR}/logs"

    install_project_file "docker-compose.single-node.yml" "${COMPOSE_DIR}/docker-compose.yml" "0644"
    install_project_file ".env.example" "${COMPOSE_DIR}/.env.example" "0644"
    install_project_file "update.sh" "${COMPOSE_DIR}/update.sh" "0755"
    install_generate_keys_script "${COMPOSE_DIR}/generate_keys.sh"

    validate_managed_regular_file "${COMPOSE_DIR}/.env" false
    if [[ -f "${COMPOSE_DIR}/.env" ]]; then
        warn "keeping existing ${COMPOSE_DIR}/.env"
    else
        local generated_compose_env
        info "generating ${COMPOSE_DIR}/.env"
        generated_compose_env="$(mktemp)"
        generate_compose_single_node_env "${generated_compose_env}"
        atomic_install_managed_file \
            "${generated_compose_env}" "${COMPOSE_DIR}/.env" 0600
        rm -f -- "${generated_compose_env}"
    fi


    cat <<EOF

Docker Compose single-node files are ready:
  ${COMPOSE_DIR}/docker-compose.yml
  ${COMPOSE_DIR}/.env
  ${COMPOSE_DIR}/.env.example
  ${COMPOSE_DIR}/update.sh
  ${COMPOSE_DIR}/generate_keys.sh
  ${COMPOSE_DIR}/logs
EOF

    if [[ "${SKIP_START}" == "true" ]]; then
        compose_manual_start_steps
        return
    fi

    require_compose_runtime
    start_compose_deployment
    compose_next_steps
}

install_env_file() {
    local env_file="$1"
    install_config_dir

    if [[ -n "${env_file}" ]]; then
        info "installing env file to ${ENV_TARGET}"
        install_env_target_from "${env_file}"
    else
        ensure_env_target_permissions
    fi
    replace_or_append_env "${ENV_TARGET}" "AETHER_UPDATE_STRATEGY" "manual"
}

switch_current_release_link() {
    local release_dir="$1"
    local current_link="$2"
    local next_link="${current_link}.new"

    [[ ! -e "${current_link}" || -L "${current_link}" ]] \
        || die "current release path exists and is not a symbolic link"
    [[ ! -e "${next_link}" || -L "${next_link}" ]] \
        || die "temporary current release path exists and is not a symbolic link"

    rm -f -- "${next_link}"
    ln -s -- "${release_dir}" "${next_link}"
    [[ -L "${next_link}" && "$(readlink "${next_link}")" == "${release_dir}" ]] \
        || die "could not create the temporary current release symbolic link"

    # Recheck immediately before rename. The install root is root-owned and not
    # writable by the service account, so an unprivileged process cannot race it.
    [[ ! -e "${current_link}" || -L "${current_link}" ]] \
        || die "current release path changed to a non-symbolic-link"
    case "$(install_os)" in
        linux)
            mv -fT -- "${next_link}" "${current_link}"
            ;;
        macos)
            mv -fh -- "${next_link}" "${current_link}"
            ;;
    esac
}

select_release_install_directory() {
    local requested="$1"
    local selected

    if [[ ! -e "${requested}" && ! -L "${requested}" ]]; then
        printf '%s\n' "${requested}"
        return
    fi
    [[ -d "${requested}" && ! -L "${requested}" ]] \
        || die "release path exists and is not a managed directory: ${requested}"

    selected="$(mktemp -d "${requested}.XXXXXXXX")" \
        || die "could not allocate an immutable release directory beside ${requested}"
    [[ -d "${selected}" && ! -L "${selected}" ]] \
        || die "release staging path is not a real directory: ${selected}"
    printf '%s\n' "${selected}"
}

install_release() {
    local bundle="$1"
    local release_dir
    local current_link="${INSTALL_ROOT}/current"

    validate_release_identifier "${VERSION}"
    release_dir="${INSTALL_ROOT}/releases/${VERSION}"
    validate_local_bundle_tree "${bundle}"

    [[ ! -L "${INSTALL_ROOT}" ]] || die "install root may not be a symbolic link: ${INSTALL_ROOT}"
    if [[ -e "${INSTALL_ROOT}" && ! -d "${INSTALL_ROOT}" ]]; then
        die "install root path is not a directory: ${INSTALL_ROOT}"
    fi
    [[ ! -L "${INSTALL_ROOT}/releases" ]] \
        || die "releases directory may not be a symbolic link: ${INSTALL_ROOT}/releases"
    [[ ! -L "${INSTALL_ROOT}/data" ]] \
        || die "data directory may not be a symbolic link: ${INSTALL_ROOT}/data"
    [[ ! -L "${INSTALL_ROOT}/logs" ]] \
        || die "logs directory may not be a symbolic link: ${INSTALL_ROOT}/logs"
    install -d -o root -m 0755 "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases"
    [[ ! -L "${INSTALL_ROOT}" ]] || die "install root became a symbolic link: ${INSTALL_ROOT}"
    [[ ! -L "${INSTALL_ROOT}/releases" ]] \
        || die "releases directory became a symbolic link: ${INSTALL_ROOT}/releases"
    chmod 0755 "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases"
    install -d -m 0755 "${INSTALL_ROOT}/data" "${INSTALL_ROOT}/logs"
    [[ ! -L "${INSTALL_ROOT}/data" && ! -L "${INSTALL_ROOT}/logs" ]] \
        || die "managed data or log directory became a symbolic link"
    if is_darwin; then
        install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" -m 0750 \
            "${INSTALL_ROOT}/data" \
            "${INSTALL_ROOT}/logs"
    else
        install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" -m 0750 \
            "${INSTALL_ROOT}/data" \
            "${INSTALL_ROOT}/logs"
    fi
    release_dir="$(select_release_install_directory "${release_dir}")"
    info "installing release ${VERSION} into ${release_dir}"
    install -d -m 0755 "${release_dir}/bin" "${release_dir}/frontend"
    cp -P "${bundle}/bin/aether-gateway" "${release_dir}/bin/aether-gateway"
    cp -RP "${bundle}/frontend/." "${release_dir}/frontend/"
    validate_local_bundle_tree "${release_dir}"
    chmod -R u=rwX,go=rX "${release_dir}"
    if is_darwin; then
        chown -R root:"${SERVICE_GROUP}" "${release_dir}"
    else
        chown -R root:"${SERVICE_GROUP}" "${release_dir}"
    fi
    chmod -R u=rwX,go=rX "${release_dir}"
    switch_current_release_link "${release_dir}" "${current_link}"
}

prune_old_releases() {
    local keep="${RELEASE_KEEP}"
    [[ "${keep}" =~ ^[0-9]+$ ]] || return 0
    [[ "${keep}" -gt 0 ]] || return 0

    local releases_dir="${INSTALL_ROOT}/releases"
    [[ -d "${releases_dir}" ]] || return 0

    local current_target
    current_target="$(readlink "${INSTALL_ROOT}/current" 2>/dev/null || true)"
    current_target="$(basename "${current_target}" 2>/dev/null || true)"

    local releases_dir_real
    releases_dir_real="$(cd -- "${releases_dir}" && pwd -P)"
    local dir name parent_real
    local -a safe_release_dirs=()
    for dir in "${releases_dir}"/*; do
        [[ -d "${dir}" && ! -L "${dir}" ]] || continue
        name="$(basename -- "${dir}")"
        is_safe_release_identifier "${name}" || continue
        parent_real="$(cd -- "$(dirname -- "${dir}")" && pwd -P)"
        [[ "${parent_real}" == "${releases_dir_real}" ]] || continue
        [[ "${name}" != "${current_target}" ]] || continue
        safe_release_dirs+=("${dir}")
    done

    local count="${#safe_release_dirs[@]}"

    if [[ "${count}" -ge "${keep}" ]]; then
        local to_remove
        to_remove="$(ls -1dt -- "${safe_release_dirs[@]}" 2>/dev/null | tail -n +$((keep)))"

        local removed=0
        while IFS= read -r dir; do
            [[ -n "${dir}" ]] || continue
            [[ -d "${dir}" && ! -L "${dir}" ]] || continue
            name="$(basename -- "${dir}")"
            is_safe_release_identifier "${name}" || continue
            parent_real="$(cd -- "$(dirname -- "${dir}")" && pwd -P)"
            [[ "${parent_real}" == "${releases_dir_real}" ]] || continue
            info "pruning old release: $(basename "${dir}")"
            rm -rf -- "${dir}"
            removed=$((removed + 1))
        done <<< "${to_remove}"

        if [[ "${removed}" -gt 0 ]]; then
            if ui_is_zh; then
                info "已清理 ${removed} 个旧版本（保留最新 ${keep} 个）"
            else
                info "pruned ${removed} old release(s), keeping latest ${keep}"
            fi
        fi
    fi
}

render_systemd_unit() {
    cat <<EOF
[Unit]
Description=Aether Gateway
Documentation=https://github.com/${REPO}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_GROUP}
WorkingDirectory=${INSTALL_ROOT}/current
EnvironmentFile=${ENV_TARGET}
ExecStart=${INSTALL_ROOT}/current/bin/aether-gateway
Restart=on-failure
RestartSec=3
TimeoutStopSec=20
UMask=0027
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
}

install_systemd_unit() {
    local rendered_unit unit_dir
    rendered_unit="$(mktemp)"
    render_systemd_unit > "${rendered_unit}"
    info "installing systemd unit to ${SYSTEMD_UNIT_PATH}"
    unit_dir="$(dirname -- "${SYSTEMD_UNIT_PATH}")"
    ensure_privileged_directory "${unit_dir}" 0755 root root
    atomic_install_managed_file \
        "${rendered_unit}" "${SYSTEMD_UNIT_PATH}" 0644 root root
    rm -f -- "${rendered_unit}"
    systemctl daemon-reload
    systemctl enable "${SERVICE_NAME}" >/dev/null
}

restart_service_if_requested() {
    if [[ "${SKIP_START}" == "true" ]]; then
        info "skipping service restart"
        return
    fi

    info "restarting ${SERVICE_NAME}"
    systemctl restart "${SERVICE_NAME}"
}

print_systemd_next_steps() {
    local gateway_port
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"

    cat <<EOF

Install complete.

Gateway service:
  sudo systemctl status ${SERVICE_NAME} --no-pager
  sudo journalctl -u ${SERVICE_NAME} -n 100 --no-pager
  sudo journalctl -u ${SERVICE_NAME} -f

Health checks:
  curl -fsS http://127.0.0.1:${gateway_port}/_gateway/health
  curl -fsS http://127.0.0.1:${gateway_port}/readyz

Install directory:
  ${INSTALL_ROOT}
  data: ${INSTALL_ROOT}/data
  logs: ${INSTALL_ROOT}/logs

EOF


    cat <<EOF
Database:
  schema migrations and data backfills are prepared automatically before startup

Current release:
  ${INSTALL_ROOT}/current
EOF
}

launchd_wrapper_path() {
    printf '%s/bin/%s-launchd\n' "${INSTALL_ROOT}" "${SERVICE_NAME}"
}

install_launchd_support_files() {
    install_config_dir
    write_generate_keys_script "${CONFIG_DIR}/generate_keys.sh"
}

write_launchd_wrapper() {
    local wrapper wrapper_dir rendered
    wrapper="$(launchd_wrapper_path)"
    wrapper_dir="$(dirname -- "${wrapper}")"
    ensure_privileged_directory "${wrapper_dir}" 0755 root wheel
    rendered="$(mktemp)"
    {
        cat <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

EOF
        printf 'ENV_TARGET=%q\n' "${ENV_TARGET}"
        printf 'AETHER_BIN=%q\n' "${INSTALL_ROOT}/current/bin/aether-gateway"
        cat <<'EOF'

trim_whitespace() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "${value}"
}

strip_optional_quotes() {
    local value="$1"
    if [[ ${#value} -ge 2 ]]; then
        if [[ "${value:0:1}" == "\"" && "${value: -1}" == "\"" ]]; then
            value="${value:1:${#value}-2}"
        elif [[ "${value:0:1}" == "'" && "${value: -1}" == "'" ]]; then
            value="${value:1:${#value}-2}"
        fi
    fi
    printf '%s' "${value}"
}

if [[ ! -r "${ENV_TARGET}" ]]; then
    echo "Aether env file not found or not readable: ${ENV_TARGET}" >&2
    exit 1
fi

while IFS= read -r raw_line || [[ -n "${raw_line}" ]]; do
    line="${raw_line%$'\r'}"
    line="$(trim_whitespace "${line}")"
    [[ -z "${line}" ]] && continue
    [[ "${line:0:1}" == "#" ]] && continue

    if [[ "${line}" == export\ * || ! "${line}" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; then
        echo "Invalid Aether env line: ${line}" >&2
        exit 1
    fi

    key="${line%%=*}"
    value="${line#*=}"
    value="$(strip_optional_quotes "${value}")"
    export "${key}=${value}"
done < "${ENV_TARGET}"

exec "${AETHER_BIN}"
EOF
    } > "${rendered}"
    atomic_install_managed_file "${rendered}" "${wrapper}" 0755 root wheel
    rm -f -- "${rendered}"
}

xml_escape() {
    local value="$1"
    value="${value//&/&amp;}"
    value="${value//</&lt;}"
    value="${value//>/&gt;}"
    value="${value//\"/&quot;}"
    value="${value//\'/&apos;}"
    printf '%s' "${value}"
}

render_launchd_plist() {
    local wrapper label service_user service_group working_directory stdout_path stderr_path
    wrapper="$(xml_escape "$(launchd_wrapper_path)")"
    label="$(xml_escape "${LAUNCHD_LABEL}")"
    service_user="$(xml_escape "${SERVICE_USER}")"
    service_group="$(xml_escape "${SERVICE_GROUP}")"
    working_directory="$(xml_escape "${INSTALL_ROOT}/current")"
    stdout_path="$(xml_escape "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log")"
    stderr_path="$(xml_escape "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log")"
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${wrapper}</string>
    </array>
    <key>UserName</key>
    <string>${service_user}</string>
    <key>GroupName</key>
    <string>${service_group}</string>
    <key>WorkingDirectory</key>
    <string>${working_directory}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${stdout_path}</string>
    <key>StandardErrorPath</key>
    <string>${stderr_path}</string>
    <key>Umask</key>
    <integer>23</integer>
</dict>
</plist>
EOF
}

install_launchd_log_files() {
    local path staged
    local -a log_paths=(
        "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
        "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"
    )
    ensure_privileged_directory "${LAUNCHD_LOG_DIR}" 0755 root wheel
    for path in "${log_paths[@]}"; do
        validate_managed_regular_file "${path}" false
    done
    for path in "${log_paths[@]}"; do
        if [[ ! -e "${path}" ]]; then
            staged="$(mktemp)"
            atomic_install_managed_file \
                "${staged}" "${path}" 0640 "${SERVICE_USER}" "${SERVICE_GROUP}"
            rm -f -- "${staged}"
        else
            chown "${SERVICE_USER}:${SERVICE_GROUP}" "${path}"
            chmod 0640 "${path}"
            validate_managed_regular_file "${path}" false
        fi
    done
}

install_launchd_unit() {
    local rendered_plist plist_dir
    rendered_plist="$(mktemp)"
    render_launchd_plist > "${rendered_plist}"
    info "installing launchd plist to ${LAUNCHD_PLIST_PATH}"
    plist_dir="$(dirname -- "${LAUNCHD_PLIST_PATH}")"
    ensure_privileged_directory "${plist_dir}" 0755 root wheel
    validate_managed_regular_file "${LAUNCHD_PLIST_PATH}" true
    install_launchd_log_files
    atomic_install_managed_file \
        "${rendered_plist}" "${LAUNCHD_PLIST_PATH}" 0644 root wheel
    rm -f -- "${rendered_plist}"
}

restart_launchd_if_requested() {
    if [[ "${SKIP_START}" == "true" ]]; then
        info "skipping launchd service restart"
        return
    fi

    info "restarting ${LAUNCHD_LABEL} with launchd"
    launchctl bootout system "${LAUNCHD_PLIST_PATH}" >/dev/null 2>&1 || true
    launchctl bootstrap system "${LAUNCHD_PLIST_PATH}"
    launchctl kickstart -k "system/${LAUNCHD_LABEL}"
}

print_launchd_next_steps() {
    local gateway_port
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"

    cat <<EOF

Install complete.

Gateway service:
  sudo launchctl print system/${LAUNCHD_LABEL}
  sudo launchctl kickstart -k system/${LAUNCHD_LABEL}
  sudo launchctl bootout system ${LAUNCHD_PLIST_PATH}

Logs:
  tail -f ${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log ${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log

Health checks:
  curl -fsS http://127.0.0.1:${gateway_port}/_gateway/health
  curl -fsS http://127.0.0.1:${gateway_port}/readyz

Install directory:
  ${INSTALL_ROOT}
  data: ${INSTALL_ROOT}/data
  logs: ${INSTALL_ROOT}/logs

EOF


    cat <<EOF
Database:
  schema migrations and data backfills are prepared automatically before startup

Current release:
  ${INSTALL_ROOT}/current
EOF
}

install_systemd_mode() {
    local bundle="$1"
    local env_file="$2"

    ensure_service_account
    install_systemd_support_files
    install_env_file "${env_file}"
    validate_env_file "${ENV_TARGET}"
    install_release "${bundle}"
    prune_old_releases
    install_systemd_unit
    restart_service_if_requested
    print_systemd_next_steps
}

install_launchd_mode() {
    local bundle="$1"
    local env_file="$2"

    ensure_macos_service_account
    install_launchd_support_files
    install_env_file "${env_file}"
    validate_env_file "${ENV_TARGET}"
    install_release "${bundle}"
    prune_old_releases
    write_launchd_wrapper
    install_launchd_unit
    restart_launchd_if_requested
    print_launchd_next_steps
}

main() {
    local bundle env_file

    parse_args "$@"
    select_language
    require_supported_os
    apply_platform_defaults
    select_version
    validate_installer_source_identifiers
    select_mode

    if [[ "${MODE}" == "compose" ]]; then
        resolve_compose_release_identity
        install_compose_mode
    elif [[ "${MODE}" == "compose-single-node" ]]; then
        resolve_compose_release_identity
        install_compose_single_node_mode
    else
        require_root
        require_service_manager
        validate_single_node_managed_paths
        bundle="$(local_bundle_dir || true)"
        if [[ -z "${bundle}" ]]; then
            download_or_unpack_bundle
            bundle="${BUNDLE_DIR}"
        else
            validate_local_bundle_tree "${bundle}"
            if [[ -z "${VERSION}" ]]; then
                VERSION="$(derive_local_bundle_version "${bundle}")"
            fi
            validate_release_identifier "${VERSION}"
            info "installing from local extracted bundle ${bundle}"
        fi

        if is_darwin; then
            ensure_macos_service_account
        fi
        env_file="$(resolve_service_env_source "${MODE}")"
        case "$(install_os)" in
            linux)
                install_systemd_mode "${bundle}" "${env_file}"
                ;;
            macos)
                install_launchd_mode "${bundle}" "${env_file}"
                ;;
        esac
    fi

    if [[ -n "${ADMIN_PASSWORD_SOURCE}" ]]; then
        local password_note
        if [[ "${ADMIN_PASSWORD_SOURCE}" == "prompt" ]]; then
            password_note="set from prompt"
        else
            password_note="set from ADMIN_PASSWORD"
        fi
        cat <<EOF

Initial admin:
  username: admin
  password: ${password_note}

The password is stored in the generated env file. Change it after first login.
EOF
    fi
}

if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then
    main "$@"
fi
