#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
INSTALLER="${REPO_ROOT}/apps/aether-tunnel/install.sh"
POWERSHELL_INSTALLER="${REPO_ROOT}/apps/aether-tunnel/install.ps1"
TEST_ROOT="${REPO_ROOT}/.tmp-tunnel-installer-test.$$"

cleanup_test_root() {
    chmod -R u+rwX "${TEST_ROOT}" 2>/dev/null || true
    rm -rf -- "${TEST_ROOT}"
}
trap cleanup_test_root EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_rejected() {
    if ("$@") >/dev/null 2>&1; then
        fail_test "unsafe installer fixture was accepted: $*"
    fi
}

file_mode() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"
}

mkdir -m 700 "${TEST_ROOT}"
LIB="${TEST_ROOT}/installer-lib.sh"
sed '/^main()/,$d' "${INSTALLER}" >"${LIB}"
# shellcheck source=/dev/null
source "${LIB}"
trap cleanup_test_root EXIT

validate_release_repo "fawney19/Aether"
validate_tunnel_release_tag "tunnel-v0.3.16-rc.1"
validate_tunnel_release_tag "tunnel-v1.2.3+build.7"
validate_release_asset_name "aether-tunnel-linux-amd64.tar.gz"
validate_node_name "Tokyo edge 01"
validate_node_name "东京节点"
assert_rejected validate_release_repo "owner/repo/extra"
assert_rejected validate_release_repo 'owner/repo;touch injected'
assert_rejected validate_tunnel_release_tag "../tunnel-v0.3.16"
assert_rejected validate_tunnel_release_tag "gateway-v0.3.16"
assert_rejected validate_tunnel_release_tag "tunnel-v1.2"
assert_rejected validate_tunnel_release_tag "tunnel-v01.2.3"
assert_rejected validate_tunnel_release_tag "tunnel-v1.2.3-01"
assert_rejected validate_release_asset_name "../aether-tunnel-linux-amd64.tar.gz"
assert_rejected validate_node_name " trailing "
assert_rejected validate_node_name $'node\tname'
assert_rejected validate_node_name $'node\nname'
assert_rejected validate_node_name "$(printf 'n%.0s' {1..256})"

assert_rejected validate_https_download_url "http://example.test/release.tar.gz"
assert_rejected validate_https_download_url "https://user:pass@example.test/release.tar.gz"
assert_rejected validate_https_download_url "https://example.test/release.tar.gz#fragment"
validate_trusted_github_download_url "https://github.com/fawney19/Aether/releases/download/tunnel-v1.2.3/aether-tunnel-linux-amd64.tar.gz"
validate_trusted_github_download_url "https://release-assets.githubusercontent.com/example/object"
assert_rejected validate_trusted_github_download_url "https://github.com.evil.example/release.tar.gz"
assert_rejected validate_trusted_github_download_url "https://github.com:444/release.tar.gz"

CURL_ARGS="${TEST_ROOT}/curl-args"
curl() {
    printf '%s\n' "$@" >"${CURL_ARGS}"
    local headers='' output='' url=''
    while (($#)); do
        case "$1" in
            --dump-header) headers="$2"; shift 2 ;;
            --output) output="$2"; shift 2 ;;
            --write-out) shift 2 ;;
            -*) shift ;;
            *) url="$1"; shift ;;
        esac
    done
    : >"${headers}"
    printf '%s\n' "verified download from ${url}" >"${output}"
    printf '200'
}
download "https://github.com/fawney19/Aether/release.tar.gz" "${TEST_ROOT}/downloaded"
grep -Fxq -- '--proto' "${CURL_ARGS}" || fail_test "curl HTTPS protocol restriction is missing"
grep -Fxq -- '=https' "${CURL_ARGS}" || fail_test "curl HTTPS protocol value is missing"
grep -Fxq -- '--max-redirs' "${CURL_ARGS}" || fail_test "curl automatic redirects were not disabled"
unset -f curl

checksum_asset="aether-tunnel-linux-amd64.tar.gz"
checksum_archive="${TEST_ROOT}/${checksum_asset}"
checksum_manifest="${TEST_ROOT}/SHA256SUMS.txt"
printf '%s\n' "verified release payload" >"${checksum_archive}"
if command -v sha256sum >/dev/null 2>&1; then
    checksum="$(sha256sum "${checksum_archive}" | awk '{print $1}')"
else
    checksum="$(shasum -a 256 "${checksum_archive}" | awk '{print $1}')"
fi
printf '%s  %s\n' "${checksum}" "${checksum_asset}" >"${checksum_manifest}"
verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%064d  %s\n' 0 "${checksum_asset}" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%063d  %s\n' 0 "${checksum_asset}" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%s  %s\n' "${checksum}" "other-asset.tar.gz" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%s  %s trailing-field\n' "${checksum}" "${checksum_asset}" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%s extra-field %s\n' "${checksum}" "${checksum_asset}" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"
printf '%s  %s\n%s *%s\n' \
    "${checksum}" "${checksum_asset}" "${checksum}" "${checksum_asset}" >"${checksum_manifest}"
assert_rejected verify_checksum "${checksum_archive}" "${checksum_manifest}" "${checksum_asset}"

CONFIG_PATH="${TEST_ROOT}/normal/aether-tunnel.toml"
append_server_config "https://aether.example" "secret-token" "node-one" "off" ""
[[ "$(file_mode "${CONFIG_PATH}")" == "600" ]] || fail_test "config mode is not 0600"
grep -Fq 'management_token = "secret-token"' "${CONFIG_PATH}" || fail_test "config block was not written"

source_binary="${TEST_ROOT}/source-aether-tunnel"
printf '#!/bin/sh\nexit 0\n' >"${source_binary}"
chmod 755 "${source_binary}"
INSTALL_DIR="${TEST_ROOT}/install-bin"
install_tunnel_binary_file "${source_binary}"
[[ -x "${INSTALL_DIR}/aether-tunnel" ]] || fail_test "tunnel binary was not installed"

binary_victim="${TEST_ROOT}/binary-victim"
printf '%s\n' "keep-binary-victim" >"${binary_victim}"
rm -f "${INSTALL_DIR}/aether-tunnel"
ln -s "${binary_victim}" "${INSTALL_DIR}/aether-tunnel"
if (install_tunnel_binary_file "${source_binary}") 2>/dev/null; then
    fail_test "symbolic-link binary target was accepted"
fi
[[ "$(cat "${binary_victim}")" == "keep-binary-victim" ]] \
    || fail_test "symbolic-link binary target was modified"

rm -f "${INSTALL_DIR}/aether-tunnel"
binary_hardlink_victim="${TEST_ROOT}/binary-hardlink-victim"
printf '%s\n' "keep-hardlink-victim" >"${binary_hardlink_victim}"
ln "${binary_hardlink_victim}" "${INSTALL_DIR}/aether-tunnel"
if (install_tunnel_binary_file "${source_binary}") 2>/dev/null; then
    fail_test "hard-linked binary target was accepted"
fi
[[ "$(cat "${binary_hardlink_victim}")" == "keep-hardlink-victim" ]] \
    || fail_test "hard-linked binary victim was modified"

linked_install_target="${TEST_ROOT}/linked-install-target"
mkdir -m 700 "${linked_install_target}"
INSTALL_DIR="${TEST_ROOT}/linked-install-dir"
ln -s "${linked_install_target}" "${INSTALL_DIR}"
if (install_tunnel_binary_file "${source_binary}") 2>/dev/null; then
    fail_test "symbolic-link install directory was accepted"
fi
[[ ! -e "${linked_install_target}/aether-tunnel" ]] \
    || fail_test "symbolic-link install directory target was modified"

ancestor_target="${TEST_ROOT}/ancestor-target"
mkdir -m 700 "${ancestor_target}"
ancestor_link="${TEST_ROOT}/ancestor-link"
ln -s "${ancestor_target}" "${ancestor_link}"
INSTALL_DIR="${ancestor_link}/nested/bin"
if (install_tunnel_binary_file "${source_binary}") 2>/dev/null; then
    fail_test "symbolic-link install ancestor was accepted"
fi
[[ ! -e "${ancestor_target}/nested/bin/aether-tunnel" ]] \
    || fail_test "symbolic-link install ancestor target was modified"

chmod 644 "${CONFIG_PATH}"
append_server_config "https://aether.example" "second-token" "node-two" "off" ""
[[ "$(file_mode "${CONFIG_PATH}")" == "600" ]] || fail_test "existing config was not protected before update"
backup="$(find "$(dirname "${CONFIG_PATH}")" -maxdepth 1 -type f -name 'aether-tunnel.toml.bak.*' | head -n1)"
[[ -n "${backup}" && "$(file_mode "${backup}")" == "600" ]] || fail_test "backup mode is not 0600"

victim="${TEST_ROOT}/victim.txt"
printf '%s\n' "keep-me" >"${victim}"
CONFIG_PATH="${TEST_ROOT}/linked.toml"
ln -s "${victim}" "${CONFIG_PATH}"
if (append_server_config "https://aether.example" "stolen-token" "node-link" "off" "") 2>/dev/null; then
    fail_test "symbolic-link config was accepted"
fi
[[ "$(cat "${victim}")" == "keep-me" ]] || fail_test "symbolic-link target was modified"

hardlink_victim="${TEST_ROOT}/hardlink-victim.toml"
printf '%s\n' 'victim = true' >"${hardlink_victim}"
CONFIG_PATH="${TEST_ROOT}/hardlinked.toml"
ln "${hardlink_victim}" "${CONFIG_PATH}"
if (append_server_config "https://aether.example" "stolen-token" "node-hardlink" "off" "") 2>/dev/null; then
    fail_test "hard-linked config was accepted"
fi
[[ "$(cat "${hardlink_victim}")" == 'victim = true' ]] || fail_test "hard-linked config victim was modified"

grep -Fq 'Get-Content -LiteralPath' "${POWERSHELL_INSTALLER}" || fail_test "PowerShell reads are not literal-path safe"
grep -Fq '[IO.File]::Replace' "${POWERSHELL_INSTALLER}" || fail_test "PowerShell config replacement is not atomic"
grep -Fq 'FileAttributes]::ReparsePoint' "${POWERSHELL_INSTALLER}" || fail_test "PowerShell reparse-point guard is missing"
grep -Fq 'Assert-NoReparsePointAncestors' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell ancestor reparse-point guard is missing"
grep -Fq "LinkType -eq 'HardLink'" "${POWERSHELL_INSTALLER}" || fail_test "PowerShell hard-link guard is missing"
grep -Fq 'AllowAutoRedirect = $false' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell automatic redirects are still enabled"
grep -Fq 'Assert-TrustedGithubUri $CurrentUri' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell trusted redirect validation is missing"
if grep -Eq 'Invoke-(WebRequest|RestMethod)' "${POWERSHELL_INSTALLER}"; then
    fail_test "PowerShell installer still uses an automatically redirecting download command"
fi
grep -Fq '[IO.File]::Replace($TempBinary, $TargetBinary' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell binary replacement is not atomic"
grep -Fq '$ExpectedLines.Count -ne 1' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell checksum matching does not reject duplicates"
grep -Fq 'if (-not $ExpectedMatch.Success)' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell checksum matching does not reject malformed target entries"
if grep -Fq 'Select-Object -First 1' "${POWERSHELL_INSTALLER}"; then
    fail_test "PowerShell checksum matching still silently accepts duplicate entries"
fi
grep -Fq 'Assert-SafeReleaseRepo $Repo' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell release repository validation is missing"
grep -Fq 'Assert-SafeTunnelReleaseTag $Tag' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell release tag validation is missing"
grep -Fq 'Assert-SafeNodeName $NodeName' "${POWERSHELL_INSTALLER}" \
    || fail_test "PowerShell node-name validation is missing"
if grep -Eq 'Add-Content[[:space:]]+-Path' "${POWERSHELL_INSTALLER}"; then
    fail_test "PowerShell config still uses wildcard-aware Add-Content -Path"
fi

echo "PASS: tunnel installer config safety fixtures"
