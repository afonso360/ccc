#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-sqlite-adjustment-test.XXXXXX")
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source "$script_directory/source-adjustment.sh"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$script_directory/manifest.toml"
}

adjustment_file="$script_directory/adjustments/$(manifest_string test_adjustment_patch)"
expected_adjustment_sha256=$(manifest_string test_adjustment_sha256)
target_relative_path=$(manifest_string test_adjustment_target)
rationale=$(manifest_string test_adjustment_rationale)

[[ "$(sqlite_sha256_file "$adjustment_file")" == "$expected_adjustment_sha256" ]]

source_directory="$temporary_directory/source"
expected_directory="$temporary_directory/expected"
artifact_directory="$temporary_directory/artifacts"
mkdir -p \
  "$source_directory/ext/recover" \
  "$expected_directory/ext/recover"

cat >"$source_directory/$target_relative_path" <<'EOF'
# 2022 August 28
#
# The author disclaims copyright to this source code.  In place of
# a legal notice, here is a blessing:
#
#    May you do good and not evil.
#    May you share freely, never taking more than you give.
#
#***********************************************************************
#


source [file join [file dirname [info script]] recover_common.tcl]
set testprefix recoverfault


#--------------------------------------------------------------------------
proc compare_result {db1 db2 sql} {
EOF

cat >"$expected_directory/$target_relative_path" <<'EOF'
# 2022 August 28
#
# The author disclaims copyright to this source code.  In place of
# a legal notice, here is a blessing:
#
#    May you do good and not evil.
#    May you share freely, never taking more than you give.
#
#***********************************************************************
#


source [file join [file dirname [info script]] recover_common.tcl]
set testprefix recoverfault

forcedelete test.db2


#--------------------------------------------------------------------------
proc compare_result {db1 db2 sql} {
EOF

target_before_sha256=$(sqlite_sha256_file "$source_directory/$target_relative_path")
target_after_sha256=$(sqlite_sha256_file "$expected_directory/$target_relative_path")

apply_sqlite_test_adjustment \
  "$source_directory" \
  "$artifact_directory" \
  "$adjustment_file" \
  "$expected_adjustment_sha256" \
  "$target_relative_path" \
  "$target_before_sha256" \
  "$target_after_sha256" \
  "$rationale"

cmp "$source_directory/$target_relative_path" \
  "$expected_directory/$target_relative_path"
cmp "$adjustment_file" "$artifact_directory/source-adjustment.patch"
grep -Fxq "target=$target_relative_path" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq "patch_sha256=$expected_adjustment_sha256" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq "target_before_sha256=$target_before_sha256" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq "target_after_sha256=$target_after_sha256" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq "rationale=$rationale" \
  "$artifact_directory/source-adjustment.txt"
[[ "$(grep -Fc "$target_relative_path" \
  "$artifact_directory/source-adjustment-apply.log")" -ge 2 ]]
! grep -Eq 'offset -?[0-9]|with fuzz' \
  "$artifact_directory/source-adjustment-apply.log"

offset_source="$temporary_directory/offset-source"
offset_artifacts="$temporary_directory/offset-artifacts"
mkdir -p "$offset_source/ext/recover"
cat >"$offset_source/$target_relative_path" <<'EOF'
# inserted drift
# 2022 August 28
#
# The author disclaims copyright to this source code.  In place of
# a legal notice, here is a blessing:
#
#    May you do good and not evil.
#    May you share freely, never taking more than you give.
#
#***********************************************************************
#


source [file join [file dirname [info script]] recover_common.tcl]
set testprefix recoverfault


#--------------------------------------------------------------------------
proc compare_result {db1 db2 sql} {
EOF
offset_before_sha256=$(sqlite_sha256_file "$offset_source/$target_relative_path")

set +e
offset_output=$(apply_sqlite_test_adjustment \
  "$offset_source" \
  "$offset_artifacts" \
  "$adjustment_file" \
  "$expected_adjustment_sha256" \
  "$target_relative_path" \
  "$offset_before_sha256" \
  "$target_after_sha256" \
  "$rationale" 2>&1)
offset_status=$?
set -e
[[ "$offset_status" == 1 ]]
[[ "$offset_output" == *"requires a non-exact hunk match"* ]]
grep -Fxq '# inserted drift' "$offset_source/$target_relative_path"
! grep -Fq 'forcedelete test.db2' "$offset_source/$target_relative_path"

context_source="$temporary_directory/context-source"
context_artifacts="$temporary_directory/context-artifacts"
mkdir -p "$context_source/ext/recover"
cat >"$context_source/$target_relative_path" <<'EOF'
# 2022 August 28
#
# The author disclaims copyright to this source code.  In place of
# a legal notice, here is a blessing:
#
#    May you do good and not evil.
#    May you share freely, never taking more than you give.
#
#***********************************************************************
#


source [file join [file dirname [info script]] recover_common.tcl]
set testprefix recover_fault


#--------------------------------------------------------------------------
proc compare_result {db1 db2 sql} {
EOF
context_before_sha256=$(sqlite_sha256_file "$context_source/$target_relative_path")

set +e
context_output=$(apply_sqlite_test_adjustment \
  "$context_source" \
  "$context_artifacts" \
  "$adjustment_file" \
  "$expected_adjustment_sha256" \
  "$target_relative_path" \
  "$context_before_sha256" \
  "$target_after_sha256" \
  "$rationale" 2>&1)
context_status=$?
set -e
[[ "$context_status" == 1 ]]
[[ "$context_output" == *"does not apply exactly"* ]]
grep -Fxq 'set testprefix recover_fault' \
  "$context_source/$target_relative_path"
! grep -Fq 'forcedelete test.db2' "$context_source/$target_relative_path"

echo "SQLite source-adjustment tests passed"
