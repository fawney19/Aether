#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
# shellcheck source=../install.sh
source "${REPO_ROOT}/install.sh"

TEST_ROOT="$(mktemp -d)/aether installer fixtures"
mkdir -p "${TEST_ROOT}"

cleanup_test_root() {
    rm -rf -- "$(dirname -- "${TEST_ROOT}")"
}
trap cleanup_test_root EXIT

fail_test() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_link_target() {
    local link="$1"
    local expected="$2"
    [[ -L "${link}" ]] || fail_test "${link} is not a symbolic link"
    [[ "$(readlink "${link}")" == "${expected}" ]] \
        || fail_test "${link} does not point to ${expected}"
}

assert_switch_rejected() {
    local release_dir="$1"
    local current_link="$2"
    if (switch_current_release_link "${release_dir}" "${current_link}"); then
        fail_test "unsafe current-link fixture was accepted: ${current_link}"
    fi
}

test_initial_switch() {
    local fixture="${TEST_ROOT}/initial"
    local release_dir="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${release_dir}"

    switch_current_release_link "${release_dir}" "${current_link}"

    assert_link_target "${current_link}" "${release_dir}"
    [[ ! -e "${current_link}.new" && ! -L "${current_link}.new" ]] \
        || fail_test "temporary link remained after initial switch"
}

test_existing_link_switch() {
    local fixture="${TEST_ROOT}/existing link"
    local old_release="${fixture}/release old"
    local new_release="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${old_release}" "${new_release}"
    ln -s -- "${old_release}" "${current_link}"

    switch_current_release_link "${new_release}" "${current_link}"

    assert_link_target "${current_link}" "${new_release}"
    [[ ! -e "${old_release}/current.new" && ! -L "${old_release}/current.new" ]] \
        || fail_test "temporary link was moved into the previous release directory"
}

test_dangling_links_are_replaced() {
    local fixture="${TEST_ROOT}/dangling links"
    local release_dir="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${release_dir}"
    ln -s -- "${fixture}/missing current target" "${current_link}"
    ln -s -- "${fixture}/missing temporary target" "${current_link}.new"

    switch_current_release_link "${release_dir}" "${current_link}"

    assert_link_target "${current_link}" "${release_dir}"
    [[ ! -e "${current_link}.new" && ! -L "${current_link}.new" ]] \
        || fail_test "temporary dangling link remained after switch"
}

test_current_file_is_rejected() {
    local fixture="${TEST_ROOT}/current file"
    local release_dir="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${release_dir}"
    printf '%s\n' "keep-current-file" >"${current_link}"

    assert_switch_rejected "${release_dir}" "${current_link}"

    [[ "$(cat "${current_link}")" == "keep-current-file" ]] \
        || fail_test "current file was modified"
    [[ ! -e "${current_link}.new" && ! -L "${current_link}.new" ]] \
        || fail_test "temporary link was created for an unsafe current file"
}

test_current_directory_is_rejected() {
    local fixture="${TEST_ROOT}/current directory"
    local release_dir="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${release_dir}" "${current_link}"

    assert_switch_rejected "${release_dir}" "${current_link}"

    [[ -d "${current_link}" && ! -L "${current_link}" ]] \
        || fail_test "current directory was modified"
    [[ ! -e "${current_link}/current.new" && ! -L "${current_link}/current.new" ]] \
        || fail_test "temporary link was moved into an unsafe current directory"
}

test_temporary_file_is_rejected() {
    local fixture="${TEST_ROOT}/temporary file"
    local old_release="${fixture}/release old"
    local new_release="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${old_release}" "${new_release}"
    ln -s -- "${old_release}" "${current_link}"
    printf '%s\n' "keep-temporary-file" >"${current_link}.new"

    assert_switch_rejected "${new_release}" "${current_link}"

    assert_link_target "${current_link}" "${old_release}"
    [[ "$(cat "${current_link}.new")" == "keep-temporary-file" ]] \
        || fail_test "temporary file was modified"
}

test_temporary_directory_is_rejected() {
    local fixture="${TEST_ROOT}/temporary directory"
    local old_release="${fixture}/release old"
    local new_release="${fixture}/release new"
    local current_link="${fixture}/current"
    mkdir -p "${old_release}" "${new_release}" "${current_link}.new"
    ln -s -- "${old_release}" "${current_link}"

    assert_switch_rejected "${new_release}" "${current_link}"

    assert_link_target "${current_link}" "${old_release}"
    [[ -d "${current_link}.new" && ! -L "${current_link}.new" ]] \
        || fail_test "temporary directory was modified"
}

test_managed_directory_link_is_rejected() {
    local fixture="${TEST_ROOT}/managed directory link"
    local target="${fixture}/target"
    local link="${fixture}/managed"
    mkdir -p "${target}"
    ln -s -- "${target}" "${link}"

    if (ensure_directory "${link}"); then
        fail_test "managed directory symbolic link was accepted"
    fi
}

test_existing_release_is_never_rewritten_in_place() {
    local fixture="${TEST_ROOT}/immutable release"
    local requested="${fixture}/releases/v1.2.3"
    local selected
    mkdir -p "${requested}"
    printf '%s\n' "old-release" >"${requested}/marker"

    selected="$(select_release_install_directory "${requested}")"

    [[ "${selected}" != "${requested}" ]] \
        || fail_test "an existing release directory was selected for in-place rewriting"
    [[ -d "${selected}" && ! -L "${selected}" ]] \
        || fail_test "replacement release directory was not allocated safely"
    [[ "$(cat "${requested}/marker")" == "old-release" ]] \
        || fail_test "the existing release was modified while allocating its replacement"
}

test_new_release_keeps_the_requested_version_path() {
    local fixture="${TEST_ROOT}/new release"
    local requested="${fixture}/releases/v1.2.3"
    local selected
    mkdir -p "$(dirname -- "${requested}")"

    selected="$(select_release_install_directory "${requested}")"
    [[ "${selected}" == "${requested}" ]] \
        || fail_test "a new release unexpectedly received a suffixed directory"
    [[ ! -e "${requested}" && ! -L "${requested}" ]] \
        || fail_test "release selection created the requested path before installation"
}

test_initial_switch
test_existing_link_switch
test_dangling_links_are_replaced
test_current_file_is_rejected
test_current_directory_is_rejected
test_temporary_file_is_rejected
test_temporary_directory_is_rejected
test_managed_directory_link_is_rejected
test_existing_release_is_never_rewritten_in_place
test_new_release_keeps_the_requested_version_path

echo "PASS: current release link safety fixtures"
