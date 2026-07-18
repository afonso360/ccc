#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-zstd-adjustment-test.XXXXXX")
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source "$script_directory/source-adjustment.sh"

source_directory="$temporary_directory/source"
artifact_directory="$temporary_directory/artifacts"
mkdir -p "$source_directory/lib/common"
cat >"$source_directory/lib/common/example.h" <<'EOF'
#if defined(__GNUC__)
selected_path=gnu
#else
selected_path=generic
#endif
EOF
cat >"$temporary_directory/adjustment.patch" <<'EOF'
diff --git a/lib/common/example.h b/lib/common/example.h
--- a/lib/common/example.h
+++ b/lib/common/example.h
@@ -1 +1 @@
-#if defined(__GNUC__)
+#if defined(__GNUC__) && !defined(ZSTD_DISABLE_ASM)
EOF

before=$(zstd_sha256_file "$source_directory/lib/common/example.h")
cat >"$temporary_directory/expected.h" <<'EOF'
#if defined(__GNUC__) && !defined(ZSTD_DISABLE_ASM)
selected_path=gnu
#else
selected_path=generic
#endif
EOF
after=$(zstd_sha256_file "$temporary_directory/expected.h")
cat >"$temporary_directory/hashes" <<EOF
# path before_sha256 after_sha256
lib/common/example.h $before $after
EOF
patch_hash=$(zstd_sha256_file "$temporary_directory/adjustment.patch")
hash_list_hash=$(zstd_sha256_file "$temporary_directory/hashes")

apply_zstd_source_adjustment \
  "$source_directory" \
  "$artifact_directory" \
  "$temporary_directory/adjustment.patch" \
  "$patch_hash" \
  "$temporary_directory/hashes" \
  "$hash_list_hash" \
  "test generic path"

cmp "$source_directory/lib/common/example.h" "$temporary_directory/expected.h"
grep -Fxq "patch_sha256=$patch_hash" "$artifact_directory/source-adjustment.txt"
grep -Fxq "hash_list_sha256=$hash_list_hash" "$artifact_directory/source-adjustment.txt"
grep -Fxq 'rationale=test generic path' "$artifact_directory/source-adjustment.txt"
! grep -Eq 'offset -?[0-9]|with fuzz' "$artifact_directory/source-adjustment-apply.log"

drift_source="$temporary_directory/drift-source"
mkdir -p "$drift_source/lib/common"
cat >"$drift_source/lib/common/example.h" <<'EOF'
/* drift */
#if defined(__GNUC__)
selected_path=gnu
#else
selected_path=generic
#endif
EOF
drift_before=$(zstd_sha256_file "$drift_source/lib/common/example.h")
cat >"$temporary_directory/drift-hashes" <<EOF
lib/common/example.h $drift_before $after
EOF
drift_hash_list_hash=$(zstd_sha256_file "$temporary_directory/drift-hashes")
set +e
drift_output=$(apply_zstd_source_adjustment \
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
grep -Fxq '/* drift */' "$drift_source/lib/common/example.h"

echo "zstd source-adjustment tests passed"
