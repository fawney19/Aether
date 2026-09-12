#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
# shellcheck source=../install.sh
source "${REPO_ROOT}/install.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_rejected() {
    if ("$@") >/dev/null 2>&1; then
        fail_test "unsafe managed-file fixture was accepted: $*"
    fi
}

assert_victim_unchanged() {
    local victim="$1"
    local expected="$2"
    [[ "$(cat "${victim}")" == "${expected}" ]] \
        || fail_test "managed-file guard modified ${victim}"
}

mkdir -p "${TEST_ROOT}/managed" "${TEST_ROOT}/config"
source_file="${TEST_ROOT}/source"
victim="${TEST_ROOT}/victim"
target="${TEST_ROOT}/managed/target"
printf '%s\n' "replacement" >"${source_file}"
printf '%s\n' "keep-victim" >"${victim}"

ln -s "${victim}" "${target}"
assert_rejected atomic_install_managed_file "${source_file}" "${target}" 0600
assert_victim_unchanged "${victim}" "keep-victim"
rm -f "${target}"

ln "${victim}" "${target}"
atomic_install_managed_file "${source_file}" "${target}" 0600
assert_victim_unchanged "${victim}" "keep-victim"
[[ "$(cat "${target}")" == "replacement" ]] \
    || fail_test "atomic managed-file replacement did not update the target"
[[ "$(stat_file_link_count "${target}")" == "1" ]] \
    || fail_test "atomic managed-file replacement retained an attacker-controlled hard link"

keys_victim="${TEST_ROOT}/keys-victim"
keys_target="${TEST_ROOT}/managed/generate_keys.sh"
printf '%s\n' "keep-keys-victim" >"${keys_victim}"
ln -s "${keys_victim}" "${keys_target}"
CONFIG_DIR="${TEST_ROOT}/config"
assert_rejected write_generate_keys_script "${keys_target}"
assert_victim_unchanged "${keys_victim}" "keep-keys-victim"

env_victim="${TEST_ROOT}/env-victim"
env_target="${TEST_ROOT}/config/aether-gateway.env"
printf '%s\n' "keep-env-victim" >"${env_victim}"
ln -s "${env_victim}" "${env_target}"
assert_rejected replace_or_append_env "${env_target}" "AETHER_UPDATE_STRATEGY" "manual"
assert_victim_unchanged "${env_victim}" "keep-env-victim"

ENV_TARGET="${env_target}"
SERVICE_GROUP="$(id -gn)"
assert_rejected install_env_target_from "${source_file}"
assert_victim_unchanged "${env_victim}" "keep-env-victim"
rm -f "${env_target}"
printf '%s\n' "AETHER_UPDATE_STRATEGY=self" >"${env_target}"
chmod 0640 "${env_target}"
replace_or_append_env "${env_target}" "AETHER_UPDATE_STRATEGY" "manual"
grep -Fxq "AETHER_UPDATE_STRATEGY=manual" "${env_target}" \
    || fail_test "env update did not replace the managed value"
[[ "$(stat_file_mode "${env_target}")" == "640" ]] \
    || fail_test "env update did not preserve the managed file mode"

# Exercise the real writers without requiring root ownership changes in the
# temporary fixture tree. The original atomic replacement implementation is
# retained; only ownership application and privileged-directory creation are
# adapted for an unprivileged test process.
eval "$(declare -f atomic_install_managed_file | \
    sed '1s/atomic_install_managed_file/original_atomic_install_managed_file/')"
atomic_install_managed_file() {
    original_atomic_install_managed_file "$1" "$2" "$3"
}
ensure_privileged_directory() {
    ensure_directory "$1" "$2"
}
systemctl() {
    return 0
}

systemd_dir="${TEST_ROOT}/systemd"
SYSTEMD_UNIT_PATH="${systemd_dir}/aether-gateway.service"
mkdir -p "${systemd_dir}"
unit_victim="${TEST_ROOT}/unit-victim"
printf '%s\n' "keep-unit-victim" >"${unit_victim}"
ln -s "${unit_victim}" "${SYSTEMD_UNIT_PATH}"
assert_rejected install_systemd_unit
assert_victim_unchanged "${unit_victim}" "keep-unit-victim"
rm -f "${SYSTEMD_UNIT_PATH}"
install_systemd_unit
[[ -f "${SYSTEMD_UNIT_PATH}" && ! -L "${SYSTEMD_UNIT_PATH}" ]] \
    || fail_test "systemd unit was not installed as a regular file"

INSTALL_ROOT="${TEST_ROOT}/install path/\$(touch wrapper-injected)"
CONFIG_DIR="${TEST_ROOT}/config"
ENV_TARGET="${TEST_ROOT}/config path/\$(touch env-injected)"
mkdir -p "$(dirname "${ENV_TARGET}")"
wrapper="$(launchd_wrapper_path)"
mkdir -p "$(dirname "${wrapper}")"
wrapper_victim="${TEST_ROOT}/wrapper-victim"
printf '%s\n' "keep-wrapper-victim" >"${wrapper_victim}"
ln -s "${wrapper_victim}" "${wrapper}"
assert_rejected write_launchd_wrapper
assert_victim_unchanged "${wrapper_victim}" "keep-wrapper-victim"
rm -f "${wrapper}"
write_launchd_wrapper
bash -n "${wrapper}"
if (cd "${TEST_ROOT}" && "${wrapper}") >/dev/null 2>&1; then
    fail_test "launchd wrapper unexpectedly ran without its env file"
fi
[[ ! -e "${TEST_ROOT}/wrapper-injected" && ! -e "${TEST_ROOT}/env-injected" ]] \
    || fail_test "launchd wrapper executed a generated-path shell injection"

LAUNCHD_LOG_DIR="${TEST_ROOT}/launchd-logs"
mkdir -p "${LAUNCHD_LOG_DIR}"
log_victim="${TEST_ROOT}/log-victim"
printf '%s\n' "keep-log-victim" >"${log_victim}"
ln -s "${log_victim}" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
assert_rejected install_launchd_log_files
assert_victim_unchanged "${log_victim}" "keep-log-victim"
rm -f "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
ln "${log_victim}" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
assert_rejected install_launchd_log_files
assert_victim_unchanged "${log_victim}" "keep-log-victim"
rm -f "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
printf '%s\n' "keep-existing-log" >"${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
chmod 0600 "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log"
ln -s "${log_victim}" "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"
assert_rejected install_launchd_log_files
[[ "$(cat "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log")" == "keep-existing-log" ]] \
    || fail_test "launchd log preflight changed an earlier valid log"
[[ "$(stat_file_mode "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.out.log")" == "600" ]] \
    || fail_test "launchd log preflight changed permissions before rejecting a later target"
rm -f "${LAUNCHD_LOG_DIR}/${SERVICE_NAME}.err.log"

plist_dir="${TEST_ROOT}/launchd"
LAUNCHD_PLIST_PATH="${plist_dir}/aether-gateway.plist"
mkdir -p "${plist_dir}"
plist_victim="${TEST_ROOT}/plist-victim"
printf '%s\n' "keep-plist-victim" >"${plist_victim}"
ln -s "${plist_victim}" "${LAUNCHD_PLIST_PATH}"
assert_rejected install_launchd_unit
assert_victim_unchanged "${plist_victim}" "keep-plist-victim"

LAUNCHD_LABEL="aether&amp-test"
SERVICE_USER="user&amp-test"
SERVICE_GROUP="group&lt-test"
rendered_plist="${TEST_ROOT}/rendered.plist"
render_launchd_plist >"${rendered_plist}"
grep -Fq '<string>aether&amp;amp-test</string>' "${rendered_plist}" \
    || fail_test "launchd label was not XML escaped"
grep -Fq '<string>user&amp;amp-test</string>' "${rendered_plist}" \
    || fail_test "launchd account was not XML escaped"

SERVICE_USER="aether"
SERVICE_GROUP="aether"
LAUNCHD_LABEL="../outside"
assert_rejected validate_launchd_label "${LAUNCHD_LABEL}"
assert_rejected validate_managed_absolute_path "install root" "/opt/aether/../outside"
assert_rejected validate_managed_absolute_path "install root" "/opt/aether path"

ancestor_target="${TEST_ROOT}/ancestor-target"
ancestor_link="${TEST_ROOT}/ancestor-link"
mkdir -p "${ancestor_target}"
ln -s "${ancestor_target}" "${ancestor_link}"
assert_rejected validate_privileged_path_ancestor "${ancestor_link}/managed"

echo "PASS: privileged installer managed-file safety fixtures"
