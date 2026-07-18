#!/usr/bin/env bash

sqlite_sha256_file() {
  openssl dgst -sha256 "$1" | awk '{print $NF}'
}

apply_sqlite_test_adjustment() {
  if (($# != 8)); then
    echo "apply_sqlite_test_adjustment expects 8 arguments" >&2
    return 2
  fi

  local source_directory=$1
  local artifact_directory=$2
  local adjustment_file=$3
  local expected_adjustment_sha256=$4
  local target_relative_path=$5
  local expected_target_before_sha256=$6
  local expected_target_after_sha256=$7
  local rationale=$8
  local target_path="$source_directory/$target_relative_path"
  local actual_adjustment_sha256
  local actual_target_sha256
  local application_log="$artifact_directory/source-adjustment-apply.log"

  [[ -d "$source_directory" ]] || {
    echo "SQLite source-adjustment directory does not exist: $source_directory" >&2
    return 1
  }
  [[ -f "$adjustment_file" ]] || {
    echo "SQLite source-adjustment patch does not exist: $adjustment_file" >&2
    return 1
  }
  [[ -f "$target_path" ]] || {
    echo "SQLite source-adjustment target does not exist: $target_relative_path" >&2
    return 1
  }

  actual_adjustment_sha256=$(sqlite_sha256_file "$adjustment_file")
  [[ "$actual_adjustment_sha256" == "$expected_adjustment_sha256" ]] || {
    echo "SQLite source-adjustment patch SHA-256 mismatch" >&2
    return 1
  }

  actual_target_sha256=$(sqlite_sha256_file "$target_path")
  [[ "$actual_target_sha256" == "$expected_target_before_sha256" ]] || {
    echo "SQLite source-adjustment target preimage SHA-256 mismatch: $target_relative_path" >&2
    return 1
  }

  mkdir -p "$artifact_directory"
  cp "$adjustment_file" "$artifact_directory/source-adjustment.patch"

  if ! patch --dry-run --batch --forward --fuzz=0 --strip=1 \
    --directory="$source_directory" --input="$adjustment_file" \
    >"$application_log" 2>&1; then
    cat "$application_log" >&2
    echo "SQLite source-adjustment patch does not apply exactly" >&2
    return 1
  fi
  if grep -Eq 'offset -?[0-9]|with fuzz' "$application_log"; then
    cat "$application_log" >&2
    echo "SQLite source-adjustment patch requires a non-exact hunk match" >&2
    return 1
  fi

  if ! patch --batch --forward --fuzz=0 --strip=1 \
    --directory="$source_directory" --input="$adjustment_file" \
    >>"$application_log" 2>&1; then
    cat "$application_log" >&2
    echo "SQLite source-adjustment patch failed after a successful dry run" >&2
    return 1
  fi

  actual_target_sha256=$(sqlite_sha256_file "$target_path")
  [[ "$actual_target_sha256" == "$expected_target_after_sha256" ]] || {
    echo "SQLite source-adjustment target postimage SHA-256 mismatch: $target_relative_path" >&2
    return 1
  }

  {
    printf 'target=%s\n' "$target_relative_path"
    printf 'patch_sha256=%s\n' "$actual_adjustment_sha256"
    printf 'target_before_sha256=%s\n' "$expected_target_before_sha256"
    printf 'target_after_sha256=%s\n' "$actual_target_sha256"
    printf 'rationale=%s\n' "$rationale"
  } >"$artifact_directory/source-adjustment.txt"
}
