#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-redis-adjustment-test.XXXXXX")
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

manifest_integer() {
  sed -n "s/^$1 = \([0-9][0-9]*\)$/\1/p" "$script_directory/manifest.toml"
}

[[ "$(redis_sha256_file "$script_directory/$(manifest_string source_adjustment_patch)")" == \
  "$(manifest_string source_adjustment_sha256)" ]]
[[ "$(redis_sha256_file "$script_directory/$(manifest_string source_adjustment_hashes)")" == \
  "$(manifest_string source_adjustment_hashes_sha256)" ]]
[[ "$(grep -Evc '^[[:space:]]*(#|$)' \
  "$script_directory/$(manifest_string source_adjustment_hashes)")" == \
  "$(manifest_integer source_adjustment_targets)" ]]
[[ "$(manifest_integer source_adjustment_classification_calls_rewritten)" == 0 ]]

source_directory="$temporary_directory/source"
expected_directory="$temporary_directory/expected"
artifact_directory="$temporary_directory/artifacts"
mkdir -p "$source_directory/src" "$source_directory/deps/example"
mkdir -p "$expected_directory/src" "$expected_directory/deps/example"

printf '%s\n' \
  'int selected(void) {' \
  '    return unsupported_expression();' \
  '}' >"$source_directory/src/example.c"
printf '%s\n' \
  '#define SELECTED_OPERATION unsupported_operation' \
  '#define UNCHANGED 1' >"$source_directory/deps/example/example.h"
printf '%s\n' \
  'int selected(void) {' \
  '    return portable_expression();' \
  '}' >"$expected_directory/src/example.c"
printf '%s\n' \
  '#define SELECTED_OPERATION portable_operation' \
  '#define UNCHANGED 1' >"$expected_directory/deps/example/example.h"

cat >"$temporary_directory/adjustment.patch" <<'EOF'
--- a/src/example.c
+++ b/src/example.c
@@ -1,3 +1,3 @@
 int selected(void) {
-    return unsupported_expression();
+    return portable_expression();
 }
--- a/deps/example/example.h
+++ b/deps/example/example.h
@@ -1,2 +1,2 @@
-#define SELECTED_OPERATION unsupported_operation
+#define SELECTED_OPERATION portable_operation
 #define UNCHANGED 1
EOF

source_before=$(redis_sha256_file "$source_directory/src/example.c")
header_before=$(redis_sha256_file "$source_directory/deps/example/example.h")
source_after=$(redis_sha256_file "$expected_directory/src/example.c")
header_after=$(redis_sha256_file "$expected_directory/deps/example/example.h")
cat >"$temporary_directory/hashes" <<EOF
# path before_sha256 after_sha256
src/example.c $source_before $source_after
deps/example/example.h $header_before $header_after
EOF
patch_hash=$(redis_sha256_file "$temporary_directory/adjustment.patch")
hash_list_hash=$(redis_sha256_file "$temporary_directory/hashes")

apply_redis_source_adjustment \
  "$source_directory" \
  "$artifact_directory" \
  "$temporary_directory/adjustment.patch" \
  "$patch_hash" \
  "$temporary_directory/hashes" \
  "$hash_list_hash" \
  "test portable operations"

cmp "$source_directory/src/example.c" "$expected_directory/src/example.c"
cmp "$source_directory/deps/example/example.h" \
  "$expected_directory/deps/example/example.h"
grep -Fxq "patch_sha256=$patch_hash" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq "hash_list_sha256=$hash_list_hash" \
  "$artifact_directory/source-adjustment.txt"
grep -Fxq 'rationale=test portable operations' \
  "$artifact_directory/source-adjustment.txt"
[[ "$(grep -c '^target=' "$artifact_directory/source-adjustment.txt")" == 2 ]]
! grep -Eq 'offset -?[0-9]|with fuzz' \
  "$artifact_directory/source-adjustment-apply.log"
[[ -s "$artifact_directory/source-adjustment-reapply.log" ]]

if patch --version 2>&1 | grep -Fq 'GNU patch'; then
  drift_source="$temporary_directory/drift-source"
  mkdir -p "$drift_source/src" "$drift_source/deps/example"
  printf '%s\n' \
    '/* inserted drift */' \
    'int selected(void) {' \
    '    return unsupported_expression();' \
    '}' >"$drift_source/src/example.c"
  printf '%s\n' \
    '#define SELECTED_OPERATION unsupported_operation' \
    '#define UNCHANGED 1' >"$drift_source/deps/example/example.h"
  drift_source_before=$(redis_sha256_file "$drift_source/src/example.c")
  drift_header_before=$(redis_sha256_file "$drift_source/deps/example/example.h")
  cat >"$temporary_directory/drift-hashes" <<EOF
src/example.c $drift_source_before $source_after
deps/example/example.h $drift_header_before $header_after
EOF
  drift_hash_list_hash=$(redis_sha256_file "$temporary_directory/drift-hashes")
  set +e
  drift_output=$(apply_redis_source_adjustment \
    "$drift_source" \
    "$temporary_directory/drift-artifacts" \
    "$temporary_directory/adjustment.patch" \
    "$patch_hash" \
    "$temporary_directory/drift-hashes" \
    "$drift_hash_list_hash" \
    "test drift" 2>&1)
  drift_status=$?
  set -e
  [[ "$drift_status" == 1 ]]
  [[ "$drift_output" == *"requires a non-exact hunk match"* ]]
  grep -Fxq '/* inserted drift */' "$drift_source/src/example.c"
  grep -Fq 'unsupported_expression' "$drift_source/src/example.c"
fi

set +e
hash_output=$(apply_redis_source_adjustment \
  "$expected_directory" \
  "$temporary_directory/hash-artifacts" \
  "$temporary_directory/adjustment.patch" \
  "0000000000000000000000000000000000000000000000000000000000000000" \
  "$temporary_directory/hashes" \
  "$hash_list_hash" \
  "test hash" 2>&1)
hash_status=$?
set -e
[[ "$hash_status" == 1 ]]
[[ "$hash_output" == *"patch SHA-256 mismatch"* ]]

echo "Redis source-adjustment tests passed"
