#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

mkdir -p "${TEST_ROOT}/bin" "${TEST_ROOT}/frontend"
printf '#!/bin/sh\nexit 0\n' >"${TEST_ROOT}/bin/aether-gateway"
chmod +x "${TEST_ROOT}/bin/aether-gateway"

definitions="$({
    awk '/^current_script_dir\(\)/ { emit=1 } emit { print } emit && /^}/ { emit=0 }' "${REPO_ROOT}/install.sh"
    awk '/^local_bundle_dir\(\)/ { emit=1 } emit { print } emit && /^}/ { emit=0 }' "${REPO_ROOT}/install.sh"
})"

if printf '%s\ncd -- %q\nif local_bundle_dir; then exit 9; fi\n' \
    "${definitions}" "${TEST_ROOT}" | bash; then
    :
else
    status=$?
    [[ "${status}" -ne 9 ]] || fail_test "stdin execution trusted a bundle from the current directory"
    fail_test "stdin trust regression fixture failed unexpectedly with status ${status}"
fi

# shellcheck source=../install.sh
source "${REPO_ROOT}/install.sh"
[[ "$(current_script_dir)" == "${REPO_ROOT}" ]] \
    || fail_test "a real installer file no longer resolves its adjacent bundle directory"

echo "PASS: installer source-directory trust boundary"
