#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
# shellcheck source=../install.sh
source "${REPO_ROOT}/install.sh"

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_generated_key_output() {
    local label="$1"
    local output="$2"
    local key value
    local -a values=()

    for key in \
        JWT_SECRET_KEY ENCRYPTION_KEY DB_PASSWORD REDIS_PASSWORD; do
        value="$(printf '%s\n' "${output}" | awk -F= -v key="${key}" '
            $1 == key { print substr($0, length(key) + 2); exit }
        ')"
        [[ "${value}" =~ ^[A-Za-z0-9_-]{40,}$ ]] \
            || fail_test "${label} did not generate a strong URL-safe ${key}"
        values+=("${value}")
    done
    [[ "$(printf '%s\n' "${values[@]}" | sort -u | wc -l | tr -d '[:space:]')" == "4" ]] \
        || fail_test "${label} reused a generated secret"
}

assert_line() {
    local file="$1"
    local expected="$2"
    grep -Fqx -- "${expected}" "${file}" \
        || fail_test "missing expected line in ${file}: ${expected}"
}

APP_DOCKERFILE="${REPO_ROOT}/Dockerfile.app"
COMPOSE_FILES=(
    "${REPO_ROOT}/docker-compose.yml"
    "${REPO_ROOT}/docker-compose.single-node.yml"
)

assert_line "${APP_DOCKERFILE}" "USER 0:0"
assert_line "${REPO_ROOT}/Dockerfile.app.local" "USER 0:0"
assert_line "${REPO_ROOT}/Dockerfile.app.release-local" "USER 0:0"
assert_line "${APP_DOCKERFILE}" "    HOME=/tmp/aether-home \\"

for compose_file in "${COMPOSE_FILES[@]}"; do
    assert_line "${compose_file}" '    user: "0:0"'
    assert_line "${compose_file}" "    read_only: true"
    assert_line "${compose_file}" "    cap_drop:"
    assert_line "${compose_file}" "      - ALL"
    assert_line "${compose_file}" "    cap_add:"
    assert_line "${compose_file}" "      - DAC_OVERRIDE"
    assert_line "${compose_file}" "      - FOWNER"
    assert_line "${compose_file}" "    security_opt:"
    assert_line "${compose_file}" "      - no-new-privileges:true"
    assert_line "${compose_file}" "    tmpfs:"
    assert_line "${compose_file}" "      - /tmp:rw,nosuid,nodev,noexec,mode=1777"
done

assert_line "${REPO_ROOT}/.env.example" "DB_PASSWORD="
assert_line "${REPO_ROOT}/.env.example" "REDIS_PASSWORD="
assert_line "${REPO_ROOT}/README.md" "chmod 600 .env"
grep -Fq '${DB_PASSWORD:?set DB_PASSWORD in .env}' "${REPO_ROOT}/docker-compose.yml" \
    || fail_test "Postgres password is not required by Compose"
grep -Fq '${REDIS_PASSWORD:?set REDIS_PASSWORD in .env}' "${REPO_ROOT}/docker-compose.yml" \
    || fail_test "Redis password is not required by Compose"
if grep -Eq '(DB_PASSWORD|REDIS_PASSWORD):?-?=?aether(_root)?([}"[:space:]]|$)' \
    "${REPO_ROOT}/docker-compose.yml" "${REPO_ROOT}/.env.example"; then
    fail_test "production Compose configuration contains a weak infrastructure password default"
fi

TEST_ROOT="$(mktemp -d)"
trap 'chmod -R u+rwX "${TEST_ROOT}" 2>/dev/null || true; rm -rf "${TEST_ROOT}"' EXIT
COMPOSE_DIR="${TEST_ROOT}/compose"
mkdir -p "${COMPOSE_DIR}"
fake_bin="${TEST_ROOT}/fake-bin"
mkdir -p "${fake_bin}"
printf '#!/usr/bin/env bash\nprintf "false\\n"\n' >"${fake_bin}/docker"
chmod 0755 "${fake_bin}/docker"
PATH="${fake_bin}:${PATH}"
cp "${REPO_ROOT}/.env.example" "${COMPOSE_DIR}/.env.example"
ADMIN_PASSWORD="test-admin-password"
APP_IMAGE="example.invalid/aether:test"
JWT_SECRET_KEY=""
ENCRYPTION_KEY=""
generated_env="${TEST_ROOT}/generated.env"
generate_compose_env "${generated_env}"
if grep -Eq '^AETHER_CONTAINER_(UID|GID)=' "${generated_env}"; then
    fail_test "installer still generates obsolete non-root container identity settings"
fi

generated_secrets=()
for key in \
    JWT_SECRET_KEY ENCRYPTION_KEY DB_PASSWORD REDIS_PASSWORD; do
    value="$(env_file_value "${generated_env}" "${key}")"
    [[ "${value}" =~ ^[A-Za-z0-9_-]{40,}$ ]] \
        || fail_test "installer did not generate a strong URL-safe ${key}"
    generated_secrets+=("${value}")
done
[[ "$(printf '%s\n' "${generated_secrets[@]}" | sort -u | wc -l | tr -d '[:space:]')" == "4" ]] \
    || fail_test "installer reused a generated secret"

generated_single_node_env="${TEST_ROOT}/generated-single-node.env"
generate_compose_single_node_env "${generated_single_node_env}"
assert_line "${generated_single_node_env}" "AETHER_DATABASE_DRIVER=postgres"
assert_line "${generated_single_node_env}" "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=single-node"
assert_generated_key_output "single-node installer" "$(cat "${generated_single_node_env}")"

(
    unset AETHER_DATABASE_URL DATABASE_URL AETHER_GATEWAY_DATA_POSTGRES_URL
    native_env="${TEST_ROOT}/native.env"
    if (generate_first_install_env "${native_env}") >/dev/null 2>&1; then
        fail_test "native installer accepted a missing PostgreSQL URL"
    fi
    for database_url in 'mysql://localhost/aether' 'sqlite::memory:' $'postgres://localhost/aether\nINJECTED=true'; do
        if (DATABASE_URL="${database_url}" generate_first_install_env "${native_env}") >/dev/null 2>&1; then
            fail_test "native installer accepted an unsupported or unsafe database URL"
        fi
    done
    [[ ! -e "${native_env}" ]] || fail_test "invalid native database config wrote an env file"
    DATABASE_URL="postgresql://localhost/aether" generate_first_install_env "${native_env}"
    assert_line "${native_env}" "AETHER_DATABASE_DRIVER=postgres"
    assert_line "${native_env}" "AETHER_DATABASE_URL=postgresql://localhost/aether"
    validate_env_file "${native_env}"
    replace_or_append_env "${native_env}" "AETHER_DATABASE_DRIVER" "sqlite"
    if (validate_env_file "${native_env}") >/dev/null 2>&1; then
        fail_test "native env validation accepted a removed driver"
    fi
    replace_or_append_env "${native_env}" "AETHER_DATABASE_DRIVER" "postgres"
    replace_or_append_env "${native_env}" "AETHER_DATABASE_URL" "sqlite::memory:"
    replace_or_append_env "${native_env}" "DATABASE_URL" "sqlite::memory:"
    if (validate_env_file "${native_env}") >/dev/null 2>&1; then
        fail_test "native env validation accepted a removed database URL"
    fi
)

assert_generated_key_output \
    "repository generate_keys.sh" "$("${REPO_ROOT}/generate_keys.sh")"
generated_key_script="${TEST_ROOT}/generated-keys.sh"
write_generate_keys_script "${generated_key_script}"
assert_generated_key_output \
    "installer-generated key script" "$("${generated_key_script}")"

injection_env="${TEST_ROOT}/injection.env"
printf 'SAFE=value\n' >"${injection_env}"
if (replace_or_append_env "${injection_env}" "SAFE" $'value\nINJECTED=true') >/dev/null 2>&1; then
    fail_test "dotenv CR/LF injection was accepted"
fi
grep -Fqx "SAFE=value" "${injection_env}" \
    || fail_test "rejected dotenv injection still modified the target"
if grep -Fq "INJECTED" "${injection_env}"; then
    fail_test "rejected dotenv injection added another variable"
fi

literal_backslash_value='literal\nINJECTED=true'
replace_or_append_env "${injection_env}" "SAFE" "${literal_backslash_value}"
grep -Fqx "SAFE=${literal_backslash_value}" "${injection_env}" \
    || fail_test "dotenv replacement interpreted a literal backslash escape"
[[ "$(wc -l <"${injection_env}" | tr -d '[:space:]')" == "1" ]] \
    || fail_test "literal backslash escape injected another dotenv line"

echo "PASS: production container runtime hardening fixtures"
