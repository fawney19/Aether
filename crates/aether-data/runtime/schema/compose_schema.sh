#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_root="$(cd "${root}/../../.." && pwd)"
schema_root="${root}/schema"
driver_schema_root="${schema_root}/drivers"
bootstrap_schema_root="${schema_root}/bootstrap/postgres"
adapters_root="${root}/../adapters"
postgres_migrations_root="${adapters_root}/postgres/migrations"
mysql_migrations_root="${adapters_root}/mysql/migrations"
sqlite_migrations_root="${adapters_root}/sqlite/migrations"

usage() {
  cat <<'USAGE'
Usage:
  bash crates/aether-data/runtime/schema/compose_schema.sh split
  bash crates/aether-data/runtime/schema/compose_schema.sh generate
  bash crates/aether-data/runtime/schema/compose_schema.sh compose
  bash crates/aether-data/runtime/schema/compose_schema.sh check

split   Regenerate schema source fragments from the current executable SQL.
generate Generate SQL fragments from logical schema definitions.
compose Rebuild executable SQL from schema source manifests.
check   Verify generated SQL is current, bootstrap sources are readable, and executable SQL matches manifests.
USAGE
}

generate_logical_schema() {
  (cd "${workspace_root}" && cargo run -q -p aether-data-schema --bin aether-schema -- generate)
}

check_logical_generated() {
  local args=()
  local path
  args+=(--require-tables-from "${postgres_migrations_root}/20260403000000_baseline.sql")
  for path in "${mysql_migrations_root}/"*.sql "${sqlite_migrations_root}/"*.sql; do
    [[ -f "${path}" ]] || continue
    args+=(--require-tables-from "${path}")
  done
  (cd "${workspace_root}" && cargo run -q -p aether-data-schema --bin aether-schema -- check "${args[@]}")
  printf 'ok generated logical schema\n'
}

manifest_path() {
  local target="$1"
  printf '%s/manifest.txt' "$(source_dir_path "${target}")"
}

source_dir_path() {
  local target="$1"
  printf '%s/%s' "${driver_schema_root}" "${target}"
}

output_path() {
  local target="$1"
  case "${target}" in
    postgres/baseline)
      printf '%s/20260403000000_baseline.sql' "${postgres_migrations_root}"
      ;;
    mysql/baseline)
      printf '%s/20260403000000_baseline.sql' "${mysql_migrations_root}"
      ;;
    sqlite/baseline)
      printf '%s/20260403000000_baseline.sql' "${sqlite_migrations_root}"
      ;;
    *)
      printf 'unknown schema target: %s\n' "${target}" >&2
      exit 2
      ;;
  esac
}

write_manifest() {
  local target="$1"
  shift
  local manifest
  manifest="$(manifest_path "${target}")"
  mkdir -p "$(dirname "${manifest}")"
  : > "${manifest}"
  local part
  for part in "$@"; do
    printf '%s\n' "${part}" >> "${manifest}"
  done
}

append_manifest() {
  local target="$1"
  shift
  local manifest
  manifest="$(manifest_path "${target}")"
  local part
  for part in "$@"; do
    printf '%s\n' "${part}" >> "${manifest}"
  done
}

split_fragment() {
  local source="$1"
  local target="$2"
  local filename="$3"
  local start_line="$4"
  local end_line="$5"
  local output
  output="$(source_dir_path "${target}")/${filename}"

  mkdir -p "$(dirname "${output}")"
  if [[ "${end_line}" == "EOF" ]]; then
    sed -n "${start_line},\$p" "${source}" > "${output}"
  else
    sed -n "${start_line},${end_line}p" "${source}" > "${output}"
  fi
  printf '%s: lines %s-%s\n' "${output#${root}/}" "${start_line}" "${end_line}"
}

compose_target_to_stdout() {
  local target="$1"
  local manifest
  manifest="$(manifest_path "${target}")"
  if [[ ! -f "${manifest}" ]]; then
    printf 'missing schema manifest: %s\n' "${manifest}" >&2
    exit 2
  fi

  local part
  while IFS= read -r part || [[ -n "${part}" ]]; do
    [[ -z "${part}" || "${part}" == \#* ]] && continue
    cat "$(source_dir_path "${target}")/${part}"
  done < "${manifest}"
}

compose_target() {
  local target="$1"
  local output
  output="$(output_path "${target}")"
  compose_target_to_stdout "${target}" > "${output}"
  printf 'composed %s from %s\n' "${output#${root}/}" "$(manifest_path "${target}")"
}

check_target() {
  local target="$1"
  local output tmp
  output="$(output_path "${target}")"
  tmp="$(mktemp)"
  compose_target_to_stdout "${target}" > "${tmp}"
  if ! diff -u "${output}" "${tmp}"; then
    rm -f "${tmp}"
    printf 'schema source does not match %s\n' "${output#${root}/}" >&2
    exit 1
  fi
  rm -f "${tmp}"
  printf 'ok %s\n' "${target}"
}

split_postgres_baseline() {
  local source="${postgres_migrations_root}/20260403000000_baseline.sql"
  local target="postgres/baseline"
  write_manifest "${target}" \
    "001_types_and_tables.sql" \
    "002_defaults.sql" \
    "003_constraints.sql" \
    "004_indexes.sql" \
    "005_foreign_keys.sql" \
    "006_footer.sql"
  split_fragment "${source}" "${target}" 001_types_and_tables.sql 1 1349
  split_fragment "${source}" "${target}" 002_defaults.sql 1350 1365
  split_fragment "${source}" "${target}" 003_constraints.sql 1366 2490
  split_fragment "${source}" "${target}" 004_indexes.sql 2491 3730
  split_fragment "${source}" "${target}" 005_foreign_keys.sql 3731 4582
  split_fragment "${source}" "${target}" 006_footer.sql 4583 EOF
}

check_bootstrap_sources() {
  local manifest="${bootstrap_schema_root}/manifest.txt"
  local tmp
  tmp="$(mktemp)"

  local part
  while IFS= read -r part || [[ -n "${part}" ]]; do
    [[ -z "${part}" || "${part}" == \#* ]] && continue
    cat "${bootstrap_schema_root}/${part}" >> "${tmp}"
  done < "${manifest}"

  rm -f "${tmp}"
  printf 'ok bootstrap/postgres source\n'
}

split_linear_baseline() {
  local driver="$1"
  local source
  if [[ "${driver}" == "mysql" ]]; then
    source="${mysql_migrations_root}/20260403000000_baseline.sql"
  else
    source="${sqlite_migrations_root}/20260403000000_baseline.sql"
  fi
  local target="${driver}/baseline"
  write_manifest "${target}" \
    "001_identity.sql" \
    "002_provider_catalog.sql" \
    "003_auth_config.sql" \
    "004_proxy_nodes.sql" \
    "005_wallet_billing.sql" \
    "006_usage.sql"

  if [[ "${driver}" == "mysql" ]]; then
    split_fragment "${source}" "${target}" 001_identity.sql 1 121
    split_fragment "${source}" "${target}" 002_provider_catalog.sql 122 417
    split_fragment "${source}" "${target}" 003_auth_config.sql 418 487
    split_fragment "${source}" "${target}" 004_proxy_nodes.sql 488 527
    split_fragment "${source}" "${target}" 005_wallet_billing.sql 528 711
    split_fragment "${source}" "${target}" 006_usage.sql 712 EOF
  else
    split_fragment "${source}" "${target}" 001_identity.sql 1 117
    split_fragment "${source}" "${target}" 002_provider_catalog.sql 118 417
    split_fragment "${source}" "${target}" 003_auth_config.sql 418 485
    split_fragment "${source}" "${target}" 004_proxy_nodes.sql 486 525
    split_fragment "${source}" "${target}" 005_wallet_billing.sql 526 728
    split_fragment "${source}" "${target}" 006_usage.sql 729 EOF
  fi
}

targets=(
  "postgres/baseline"
  "mysql/baseline"
  "sqlite/baseline"
)

cmd="${1:-}"
case "${cmd}" in
  split)
    split_postgres_baseline
    split_linear_baseline mysql
    split_linear_baseline sqlite
    ;;
  generate)
    generate_logical_schema
    ;;
  compose)
    for target in "${targets[@]}"; do
      compose_target "${target}"
    done
    ;;
  check)
    check_logical_generated
    check_bootstrap_sources
    for target in "${targets[@]}"; do
      check_target "${target}"
    done
    ;;
  -h|--help|help|"")
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
