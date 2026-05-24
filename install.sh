#!/usr/bin/env bash
set -euo pipefail

REPO="${AETHER_REPO:-fawney19/Aether}"
SOURCE_REF="${AETHER_SOURCE_REF:-main}"
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
COMPOSE_RELEASE_SQLITE_DATABASE_URL="sqlite://${COMPOSE_RELEASE_BASE_DIR}/data/aether.db"
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
  --channel CHANNEL    Release channel to resolve when --version is omitted: stable, latest, rc, or beta
                      stable/latest resolves the latest stable tag (default)
                      rc resolves the latest tag like v0.7.0-rc.1
                      beta resolves the latest tag like v0.7.0-beta.1
  --version VERSION    Exact release tag to install, for example v0.7.0-rc.1
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
trap cleanup EXIT

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
  4) 指定 tag，例如 v0.7.0-rc.1

请输入选项 [1]:
EOF
        else
            cat >/dev/tty <<'EOF'

Choose Aether version:
  1) Latest stable release
  2) Latest RC prerelease
  3) Latest beta prerelease
  4) Exact tag, for example v0.7.0-rc.1

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
        single-node|service|systemd|launchd|sqlite)
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
  2) Docker Compose 单节点部署（SQLite）
  3) 系统服务单节点部署（SQLite）

请输入选项 [3]:
EOF
        else
            cat >/dev/tty <<EOF

Choose Aether deployment mode:
  1) Docker Compose standard deployment (Postgres + Redis)
  2) Docker Compose single-node deployment (SQLite)
  3) System service single-node deployment (SQLite)

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

download_to() {
    local url="$1"
    local output="$2"
    local mode="${3:-quiet}"
    local show_progress="false"
    if [[ "${mode}" == "progress" && -t 2 ]]; then
        show_progress="true"
    fi

    if command -v curl >/dev/null 2>&1; then
        if [[ "${show_progress}" == "true" ]]; then
            curl -fL --progress-bar "${url}" -o "${output}"
        else
            curl -fsSL "${url}" -o "${output}"
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [[ "${show_progress}" == "true" ]]; then
            wget -O "${output}" "${url}"
        else
            wget -qO "${output}" "${url}"
        fi
    else
        die "curl or wget is required to download release assets"
    fi
}

download_stdout() {
    local url="$1"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "${url}"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "${url}"
    else
        die "curl or wget is required to download release metadata"
    fi
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
    script_dir="$(current_script_dir)"

    install -d -m 0755 "$(dirname "${target_path}")"
    if [[ -f "${script_dir}/${source_path}" ]]; then
        if same_path "${script_dir}/${source_path}" "${target_path}"; then
            chmod "${mode}" "${target_path}"
        else
            install -m "${mode}" "${script_dir}/${source_path}" "${target_path}"
        fi
    else
        download_to "$(raw_project_url "${source_path}")" "${target_path}"
        chmod "${mode}" "${target_path}"
    fi
}

install_generate_keys_script() {
    local target_path="$1"
    local script_dir
    script_dir="$(current_script_dir)"

    install -d -m 0755 "$(dirname "${target_path}")"
    if [[ -f "${script_dir}/generate_keys.sh" ]]; then
        if same_path "${script_dir}/generate_keys.sh" "${target_path}"; then
            chmod 0755 "${target_path}"
        else
            install -m 0755 "${script_dir}/generate_keys.sh" "${target_path}"
        fi
    else
        write_generate_keys_script "${target_path}"
    fi
}

ensure_directory() {
    local path="$1"
    local mode="${2:-0755}"
    if [[ ! -d "${path}" ]]; then
        install -d -m "${mode}" "${path}"
    fi
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
        *)
            die "unsupported release channel: ${CHANNEL}; expected stable, latest, rc, or beta"
            ;;
    esac
    echo "${tag}"
}

current_script_dir() {
    local source="${BASH_SOURCE[0]}"
    if [[ -n "${source}" && -f "${source}" ]]; then
        cd -- "$(dirname -- "${source}")" && pwd
    else
        pwd
    fi
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
    dir="$(current_script_dir)"
    if [[ -x "${dir}/bin/aether-gateway" && -d "${dir}/frontend" ]]; then
        echo "${dir}"
    fi
}

download_or_unpack_bundle() {
    TMP_ROOT="$(mktemp -d)"
    if [[ -n "${ARCHIVE_PATH}" ]]; then
        [[ -f "${ARCHIVE_PATH}" ]] || die "archive not found: ${ARCHIVE_PATH}"
        info "using local archive ${ARCHIVE_PATH}"
        tar -xzf "${ARCHIVE_PATH}" -C "${TMP_ROOT}"
    else
        local os arch
        os="$(install_os)"
        arch="$(detect_arch)"

        local tag asset base_url archive_url archive_file
        tag="$(resolve_version)"
        [[ -n "${tag}" ]] || die "could not resolve ${CHANNEL} release tag for ${REPO}"
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
        tar -xzf "${archive_file}" -C "${TMP_ROOT}"
    fi

    local bundle
    bundle="$(find "${TMP_ROOT}" -mindepth 1 -maxdepth 1 -type d | head -n1)"
    [[ -n "${bundle}" ]] || die "release archive did not contain a bundle directory"
    [[ -x "${bundle}/bin/aether-gateway" ]] || die "bundle is missing bin/aether-gateway"
    [[ -d "${bundle}/frontend" ]] || die "bundle is missing frontend"
    if [[ -z "${VERSION}" ]]; then
        VERSION="$(derive_local_bundle_version "${bundle}")"
    fi
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
    local output_dir output_dir_normalized config_dir_normalized
    output_dir="$(dirname "${output}")"
    output_dir_normalized="${output_dir%/}"
    config_dir_normalized="${CONFIG_DIR%/}"
    [[ -n "${output_dir_normalized}" ]] || output_dir_normalized="/"
    [[ -n "${config_dir_normalized}" ]] || config_dir_normalized="/"
    if is_darwin && [[ "${output_dir_normalized}" == "${config_dir_normalized}" ]]; then
        install_config_dir
    else
        install -d -m 0755 "${output_dir}"
    fi
    cat > "${output}" <<'EOF'
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
KEYS
EOF
    chmod 0755 "${output}"
}

replace_or_append_env() {
    local file="$1"
    local key="$2"
    local value="$3"

    if grep -qE "^#?[[:space:]]*${key}=" "${file}"; then
        local tmp_file
        tmp_file="$(mktemp)"
        awk -v key="${key}" -v value="${value}" '
            BEGIN {
                pattern = "^#?[[:space:]]*" key "="
                replacement = key "=" value
            }
            $0 ~ pattern && replaced == 0 {
                print replacement
                replaced = 1
                next
            }
            { print }
        ' "${file}" > "${tmp_file}"
        cat "${tmp_file}" > "${file}"
        rm -f "${tmp_file}"
    else
        printf '%s=%s\n' "${key}" "${value}" >> "${file}"
    fi
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
    local jwt_key encryption_key
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"

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
AETHER_GATEWAY_AUTO_PREPARE_DATABASE=true
AETHER_RUNTIME_BACKEND=memory
API_KEY_PREFIX=sk

AETHER_DATABASE_DRIVER=sqlite
AETHER_DATABASE_URL=sqlite://${INSTALL_ROOT}/data/aether.db
DATABASE_URL=sqlite://${INSTALL_ROOT}/data/aether.db

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
AETHER_GATEWAY_AUTO_PREPARE_DATABASE=true
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
            rc|beta)
                tag="${CHANNEL}"
                ;;
            *)
                die "unsupported release channel: ${CHANNEL}; expected stable, latest, rc, or beta"
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

compose_log_env_block() {
    cat <<EOF
AETHER_LOG_DESTINATION=${COMPOSE_LOG_DESTINATION_DEFAULT}
AETHER_LOG_FORMAT=${COMPOSE_LOG_FORMAT_DEFAULT}
AETHER_LOG_DIR=${COMPOSE_RELEASE_LOG_DIR}
AETHER_LOG_ROTATION=${COMPOSE_LOG_ROTATION_DEFAULT}
AETHER_LOG_RETENTION_DAYS=${COMPOSE_LOG_RETENTION_DAYS_DEFAULT}
AETHER_LOG_MAX_FILES=${COMPOSE_LOG_MAX_FILES_DEFAULT}
EOF
}

generate_compose_env() {
    local output="$1"
    local jwt_key encryption_key
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"

    cp "${COMPOSE_DIR}/.env.example" "${output}"
    replace_or_append_env "${output}" "APP_IMAGE" "$(compose_image)"
    replace_or_append_env "${output}" "APP_PORT" "$(compose_app_port)"
    replace_or_append_env "${output}" "DB_PASSWORD" "aether"
    replace_or_append_env "${output}" "REDIS_PASSWORD" "aether"
    replace_or_append_env "${output}" "JWT_SECRET_KEY" "${JWT_SECRET_KEY:-${jwt_key}}"
    replace_or_append_env "${output}" "ENCRYPTION_KEY" "${ENCRYPTION_KEY:-${encryption_key}}"
    replace_or_append_env "${output}" "ADMIN_EMAIL" "${ADMIN_EMAIL:-admin@example.local}"
    replace_or_append_env "${output}" "ADMIN_USERNAME" "${ADMIN_USERNAME:-admin}"
    replace_or_append_env "${output}" "ADMIN_PASSWORD" "${ADMIN_PASSWORD}"
    replace_or_append_env "${output}" "AETHER_UPDATE_STRATEGY" "docker"
    replace_or_append_env "${output}" "AETHER_DOCKER_UPDATE_COMMAND" "./update.sh"
    append_compose_log_env_defaults "${output}"
    replace_or_append_env "${output}" "AETHER_GATEWAY_AUTO_PREPARE_DATABASE" "true"
}

generate_compose_single_node_env() {
    local output="$1"
    local jwt_key encryption_key
    prompt_admin_password
    jwt_key="$(urlsafe_rand 32)"
    encryption_key="$(urlsafe_rand 32)"

    cat > "${output}" <<EOF
ENVIRONMENT=production
TZ=Asia/Shanghai
RUST_LOG=aether_gateway=info
$(compose_log_env_block)

APP_IMAGE=$(compose_image)
APP_PORT=$(compose_app_port)
AETHER_UPDATE_STRATEGY=docker
AETHER_DOCKER_UPDATE_COMMAND=./update.sh
AETHER_GATEWAY_STATIC_DIR=${COMPOSE_RELEASE_FRONTEND_DIR}
AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=rust-authoritative
AETHER_GATEWAY_AUTO_PREPARE_DATABASE=true
AETHER_RUNTIME_BACKEND=memory
API_KEY_PREFIX=sk

AETHER_DATABASE_DRIVER=sqlite
AETHER_DATABASE_URL=${COMPOSE_RELEASE_SQLITE_DATABASE_URL}
DATABASE_URL=${COMPOSE_RELEASE_SQLITE_DATABASE_URL}

JWT_SECRET_KEY=${JWT_SECRET_KEY:-${jwt_key}}
ENCRYPTION_KEY=${ENCRYPTION_KEY:-${encryption_key}}

ADMIN_EMAIL=${ADMIN_EMAIL:-admin@example.local}
ADMIN_USERNAME=${ADMIN_USERNAME:-admin}
ADMIN_PASSWORD=${ADMIN_PASSWORD}
EOF
}

install_config_dir() {
    if is_darwin; then
        install -d -o root -g "${SERVICE_GROUP}" -m 0750 "${CONFIG_DIR}"
    else
        install -d -m 0750 "${CONFIG_DIR}"
    fi
}

install_env_target_from() {
    local source="$1"
    if is_darwin; then
        install -o root -g "${SERVICE_GROUP}" -m 0640 "${source}" "${ENV_TARGET}"
    else
        install -m 0600 "${source}" "${ENV_TARGET}"
    fi
}

ensure_env_target_permissions() {
    if is_darwin && [[ -f "${ENV_TARGET}" ]]; then
        chown root:"${SERVICE_GROUP}" "${ENV_TARGET}"
        chmod 0640 "${ENV_TARGET}"
    fi
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

    local database_is_sqlite="false"
    if [[ "${database_driver}" == "sqlite" || "${database_url}" == sqlite:* ]]; then
        database_is_sqlite="true"
    fi

    if [[ "${topology}" == "multi-node" ]]; then
        [[ "${node_role}" != "all" ]] || die "multi-node deployment requires AETHER_GATEWAY_NODE_ROLE=frontdoor or background"
        [[ -n "${database_url}" ]] || die "multi-node deployment requires AETHER_DATABASE_URL, DATABASE_URL, or AETHER_GATEWAY_DATA_POSTGRES_URL"
        [[ "${database_is_sqlite}" != "true" ]] || die "multi-node deployment must use shared Postgres/MySQL, not SQLite"
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
        if [[ -z "${database_url}" && -z "${redis_url}" ]]; then
            warn "single-node env is running in minimal mode without full Postgres/Redis persistence"
        elif [[ "${database_is_sqlite}" == "true" && -z "${redis_url}" ]]; then
            info "single-node env is using SQLite with in-process runtime coordination"
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

    if [[ -f "${COMPOSE_DIR}/.env" ]]; then
        warn "keeping existing ${COMPOSE_DIR}/.env"
    else
        info "generating ${COMPOSE_DIR}/.env"
        generate_compose_env "${COMPOSE_DIR}/.env"
        chmod 0600 "${COMPOSE_DIR}/.env"
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
    ensure_directory "${COMPOSE_DIR}/data"
    ensure_directory "${COMPOSE_DIR}/logs"

    install_project_file "docker-compose.single-node.yml" "${COMPOSE_DIR}/docker-compose.yml" "0644"
    install_project_file ".env.example" "${COMPOSE_DIR}/.env.example" "0644"
    install_project_file "update.sh" "${COMPOSE_DIR}/update.sh" "0755"
    install_generate_keys_script "${COMPOSE_DIR}/generate_keys.sh"

    if [[ -f "${COMPOSE_DIR}/.env" ]]; then
        warn "keeping existing ${COMPOSE_DIR}/.env"
    else
        info "generating ${COMPOSE_DIR}/.env"
        generate_compose_single_node_env "${COMPOSE_DIR}/.env"
        chmod 0600 "${COMPOSE_DIR}/.env"
    fi

    cat <<EOF

Docker Compose single-node files are ready:
  ${COMPOSE_DIR}/docker-compose.yml
  ${COMPOSE_DIR}/.env
  ${COMPOSE_DIR}/.env.example
  ${COMPOSE_DIR}/update.sh
  ${COMPOSE_DIR}/generate_keys.sh
  ${COMPOSE_DIR}/data
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
}

install_release() {
    local bundle="$1"
    local release_dir="${INSTALL_ROOT}/releases/${VERSION}"
    local current_link="${INSTALL_ROOT}/current"

    [[ -x "${bundle}/bin/aether-gateway" ]] || die "binary not found or not executable: ${bundle}/bin/aether-gateway"
    [[ -d "${bundle}/frontend" ]] || die "frontend directory not found: ${bundle}/frontend"

    info "installing release ${VERSION} into ${release_dir}"
    install -d -m 0755 "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases"
    chown root:"${SERVICE_GROUP}" "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases"
    chmod 2775 "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases"
    install -d -m 0755 "${INSTALL_ROOT}/data" "${INSTALL_ROOT}/logs"
    if is_darwin; then
        install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" -m 0750 \
            "${INSTALL_ROOT}/data" \
            "${INSTALL_ROOT}/logs"
    else
        install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" -m 0750 \
            "${INSTALL_ROOT}/data" \
            "${INSTALL_ROOT}/logs"
    fi
    rm -rf "${release_dir}"
    install -d -m 0755 "${release_dir}/bin" "${release_dir}/frontend"
    install -m 0755 "${bundle}/bin/aether-gateway" "${release_dir}/bin/aether-gateway"
    cp -R "${bundle}/frontend/." "${release_dir}/frontend/"
    chmod -R u=rwX,g=rwX,o=rX "${release_dir}"
    if is_darwin; then
        chown -R root:"${SERVICE_GROUP}" "${release_dir}"
    else
        chown -R root:"${SERVICE_GROUP}" "${release_dir}"
    fi
    chmod -R u=rwX,g=rwX,o=rX "${release_dir}"
    ln -sfn "${release_dir}" "${current_link}"
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

    local count=0
    local dir
    while IFS= read -r dir; do
        [[ -n "${dir}" ]] || continue
        local name
        name="$(basename "${dir}")"
        [[ "${name}" != "${current_target}" ]] || continue
        count=$((count + 1))
    done < <(ls -1dt "${releases_dir}"/*/ 2>/dev/null)

    if [[ "${count}" -ge "${keep}" ]]; then
        local to_remove
        to_remove="$(ls -1dt "${releases_dir}"/*/ 2>/dev/null | while IFS= read -r d; do
            local n
            n="$(basename "${d}")"
            [[ "${n}" != "${current_target}" ]] || continue
            printf '%s\n' "${d}"
        done | tail -n +$((keep)))"

        local removed=0
        while IFS= read -r dir; do
            [[ -n "${dir}" ]] || continue
            info "pruning old release: $(basename "${dir}")"
            rm -rf "${dir}"
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
    local rendered_unit
    rendered_unit="$(mktemp)"
    render_systemd_unit > "${rendered_unit}"
    info "installing systemd unit to ${SYSTEMD_UNIT_PATH}"
    install -m 0644 "${rendered_unit}" "${SYSTEMD_UNIT_PATH}"
    rm -f "${rendered_unit}"
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
    local database_driver
    local database_url
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"
    database_driver="$(awk -F= '/^[[:space:]]*AETHER_DATABASE_DRIVER=/{print tolower($2)}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    database_url="$(awk -F= '/^[[:space:]]*(AETHER_DATABASE_URL|DATABASE_URL|AETHER_GATEWAY_DATA_POSTGRES_URL)=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"

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

    if [[ "${database_driver}" == "sqlite" || "${database_url}" == sqlite:* ]]; then
        cat <<EOF
SQLite data:
  ${database_url#sqlite://}

EOF
    fi

    cat <<EOF
Database:
  empty database: first service start auto-bootstraps to the current baseline
  later schema upgrades: ${INSTALL_ROOT}/current/bin/aether-gateway --migrate

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
    local wrapper
    wrapper="$(launchd_wrapper_path)"
    install -d -o root -g wheel -m 0755 "$(dirname "${wrapper}")"
    cat > "${wrapper}" <<EOF
#!/usr/bin/env bash
set -euo pipefail

ENV_TARGET="${ENV_TARGET}"
AETHER_BIN="${INSTALL_ROOT}/current/bin/aether-gateway"
EOF
    cat >> "${wrapper}" <<'EOF'

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
    chmod 0755 "${wrapper}"
    chown root:wheel "${wrapper}"
}

render_launchd_plist() {
    local wrapper
    wrapper="$(launchd_wrapper_path)"
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${wrapper}</string>
    </array>
    <key>UserName</key>
    <string>${SERVICE_USER}</string>
    <key>GroupName</key>
    <string>${SERVICE_GROUP}</string>
    <key>WorkingDirectory</key>
    <string>${INSTALL_ROOT}/current</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log</string>
    <key>StandardErrorPath</key>
    <string>${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log</string>
    <key>Umask</key>
    <integer>23</integer>
</dict>
</plist>
EOF
}

install_launchd_log_files() {
    install -d -o root -g wheel -m 0755 "${LAUNCHD_LOG_DIR}"
    touch "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"
    chown "${SERVICE_USER}:${SERVICE_GROUP}" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"
    chmod 0640 "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"
}

install_launchd_unit() {
    local rendered_plist
    rendered_plist="$(mktemp)"
    render_launchd_plist > "${rendered_plist}"
    info "installing launchd plist to ${LAUNCHD_PLIST_PATH}"
    install_launchd_log_files
    install -d -o root -g wheel -m 0755 "$(dirname "${LAUNCHD_PLIST_PATH}")"
    install -o root -g wheel -m 0644 "${rendered_plist}" "${LAUNCHD_PLIST_PATH}"
    rm -f "${rendered_plist}"
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
    local database_driver
    local database_url
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"
    database_driver="$(awk -F= '/^[[:space:]]*AETHER_DATABASE_DRIVER=/{print tolower($2)}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    database_url="$(awk -F= '/^[[:space:]]*(AETHER_DATABASE_URL|DATABASE_URL|AETHER_GATEWAY_DATA_POSTGRES_URL)=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"

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

    if [[ "${database_driver}" == "sqlite" || "${database_url}" == sqlite:* ]]; then
        cat <<EOF
SQLite data:
  ${database_url#sqlite://}

EOF
    fi

    cat <<EOF
Database:
  empty database: first service start auto-bootstraps to the current baseline
  later schema upgrades: ${INSTALL_ROOT}/current/bin/aether-gateway --migrate

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
    select_mode

    if [[ "${MODE}" == "compose" ]]; then
        install_compose_mode
    elif [[ "${MODE}" == "compose-single-node" ]]; then
        install_compose_single_node_mode
    else
        require_root
        require_service_manager
        bundle="$(local_bundle_dir || true)"
        if [[ -z "${bundle}" ]]; then
            download_or_unpack_bundle
            bundle="${BUNDLE_DIR}"
        else
            if [[ -z "${VERSION}" ]]; then
                VERSION="$(derive_local_bundle_version "${bundle}")"
            fi
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

main "$@"
