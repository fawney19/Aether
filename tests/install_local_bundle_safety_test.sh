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

make_bundle() {
    local bundle="$1"
    mkdir -p "${bundle}/bin" "${bundle}/frontend/assets"
    printf '#!/bin/sh\nexit 0\n' >"${bundle}/bin/aether-gateway"
    printf '<!doctype html>\n' >"${bundle}/frontend/index.html"
    printf 'asset\n' >"${bundle}/frontend/assets/app.js"
    chmod 0755 "${bundle}/bin/aether-gateway"
}

assert_rejected() {
    local bundle="$1"
    local description="$2"
    if (validate_local_bundle_tree "${bundle}"); then
        fail_test "unsafe local bundle was accepted: ${description}"
    fi
}

valid="${TEST_ROOT}/valid"
make_bundle "${valid}"
validate_local_bundle_tree "${valid}"

victim="${TEST_ROOT}/victim"
printf 'do-not-touch\n' >"${victim}"

frontend_link="${TEST_ROOT}/frontend-link"
make_bundle "${frontend_link}"
rm -rf -- "${frontend_link}/frontend"
ln -s -- "${TEST_ROOT}" "${frontend_link}/frontend"
assert_rejected "${frontend_link}" "frontend symbolic link"
[[ "$(cat "${victim}")" == "do-not-touch" ]] \
    || fail_test "frontend symbolic-link validation modified its target"

binary_link="${TEST_ROOT}/binary-link"
make_bundle "${binary_link}"
rm -f -- "${binary_link}/bin/aether-gateway"
ln -s -- "${valid}/bin/aether-gateway" "${binary_link}/bin/aether-gateway"
assert_rejected "${binary_link}" "binary symbolic link"

hardlink="${TEST_ROOT}/hardlink"
make_bundle "${hardlink}"
ln "${hardlink}/frontend/index.html" "${hardlink}/frontend/index-copy.html"
assert_rejected "${hardlink}" "multiply-linked frontend file"

fifo="${TEST_ROOT}/fifo"
make_bundle "${fifo}"
mkfifo "${fifo}/frontend/events.pipe"
assert_rejected "${fifo}" "FIFO frontend entry"

bundle_link="${TEST_ROOT}/bundle-link"
ln -s -- "${valid}" "${bundle_link}"
assert_rejected "${bundle_link}" "bundle symbolic link"

echo "PASS: local release bundle type and link safety fixtures"
