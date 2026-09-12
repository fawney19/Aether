#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

PROJECT_DIR="${TEST_ROOT}/project"
FAKE_BIN="${TEST_ROOT}/fake-bin"
mkdir -p "${PROJECT_DIR}" "${FAKE_BIN}"
cp "${REPO_ROOT}/deploy.sh" "${PROJECT_DIR}/deploy.sh"
chmod 0755 "${PROJECT_DIR}/deploy.sh"
printf 'FROM scratch\n' >"${PROJECT_DIR}/Dockerfile.app.local"
printf 'services: {}\n' >"${PROJECT_DIR}/docker-compose.yml"
printf 'services: {}\n' >"${PROJECT_DIR}/docker-compose.local.yml"

cat >"${FAKE_BIN}/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "image" && "${2:-}" == "inspect" ]]; then
    exit 1
fi
if [[ " $* " == *" ps -q "* ]]; then
    printf 'fixture-container\n'
fi
exit 0
EOF
chmod 0755 "${FAKE_BIN}/docker"
cp "${FAKE_BIN}/docker" "${FAKE_BIN}/docker-compose"

VICTIM="${TEST_ROOT}/victim"
printf 'known-good\n' >"${VICTIM}"
ln -s -- "${VICTIM}" "${PROJECT_DIR}/.code-hash"

if (cd -- "${PROJECT_DIR}" && PATH="${FAKE_BIN}:${PATH}" bash ./deploy.sh) >/dev/null 2>&1; then
    fail_test "deploy accepted a symbolic-link build-state file"
fi
[[ "$(cat -- "${VICTIM}")" == "known-good" ]] \
    || fail_test "deploy overwrote the symbolic-link target"
[[ -L "${PROJECT_DIR}/.code-hash" ]] \
    || fail_test "deploy unexpectedly replaced the rejected symbolic link"

rm -f -- "${PROJECT_DIR}/.code-hash"
(cd -- "${PROJECT_DIR}" && PATH="${FAKE_BIN}:${PATH}" bash ./deploy.sh) >/dev/null

[[ -f "${PROJECT_DIR}/.code-hash" && ! -L "${PROJECT_DIR}/.code-hash" ]] \
    || fail_test "deploy did not atomically create a regular build-state file"
mode="$(stat -c '%a' "${PROJECT_DIR}/.code-hash" 2>/dev/null || stat -f '%Lp' "${PROJECT_DIR}/.code-hash")"
[[ "${mode}" == "600" ]] || fail_test "deploy build-state mode is ${mode}, expected 600"
if find "${PROJECT_DIR}" -maxdepth 1 -name '.code-hash.tmp.*' -print -quit | grep -q .; then
    fail_test "deploy left a temporary build-state file"
fi

echo "PASS: deploy build-state symlink and atomic-write fixtures"
