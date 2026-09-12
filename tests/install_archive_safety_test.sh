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

assert_archive_rejected() {
    local archive="$1"
    if (validate_release_archive "${archive}" >/dev/null 2>&1); then
        fail_test "unsafe archive was accepted: ${archive}"
    fi
}

mkdir -p "${TEST_ROOT}/bundle/bin" "${TEST_ROOT}/bundle/frontend"
printf '#!/bin/sh\nexit 0\n' >"${TEST_ROOT}/bundle/bin/aether-gateway"
printf '<!doctype html>\n' >"${TEST_ROOT}/bundle/frontend/index.html"
chmod 0755 "${TEST_ROOT}/bundle/bin/aether-gateway"
tar -czf "${TEST_ROOT}/valid.tar.gz" -C "${TEST_ROOT}" bundle

TMP_ROOT="${TEST_ROOT}/validation"
mkdir -p "${TMP_ROOT}"
[[ "$(validate_release_archive "${TEST_ROOT}/valid.tar.gz" bundle)" == "bundle" ]] \
    || fail_test "valid release archive was rejected"

original_entry_limit="${MAX_RELEASE_ARCHIVE_ENTRIES}"
MAX_RELEASE_ARCHIVE_ENTRIES=2
assert_archive_rejected "${TEST_ROOT}/valid.tar.gz"
MAX_RELEASE_ARCHIVE_ENTRIES="${original_entry_limit}"

original_size_limit="${MAX_RELEASE_UNPACKED_BYTES}"
MAX_RELEASE_UNPACKED_BYTES=1
assert_archive_rejected "${TEST_ROOT}/valid.tar.gz"
MAX_RELEASE_UNPACKED_BYTES="${original_size_limit}"

mkdir -p "${TEST_ROOT}/linked-bundle"
ln -s /etc/passwd "${TEST_ROOT}/linked-bundle/aether-gateway"
tar -czf "${TEST_ROOT}/link.tar.gz" -C "${TEST_ROOT}" linked-bundle
assert_archive_rejected "${TEST_ROOT}/link.tar.gz"

echo "PASS: installer archive validation fixtures"
