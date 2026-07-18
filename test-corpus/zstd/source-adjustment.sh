#!/usr/bin/env bash

zstd_sha256_file() {
  openssl dgst -sha256 "$1" | awk '{print $NF}'
}

apply_zstd_source_adjustment() {
  if (($# != 7)); then
    echo "apply_zstd_source_adjustment expects 7 arguments" >&2
    return 2
  fi

  local source_directory=$1
  local artifact_directory=$2
  local adjustment_file=$3
  local expected_adjustment_sha256=$4
  local hash_file=$5
  local expected_hash_file_sha256=$6
  local rationale=$7
  local application_log="$artifact_directory/source-adjustment-apply.log"
  local actual_adjustment_sha256 actual_hash_file_sha256
  local target before after actual

  [[ -d "$source_directory" ]] || {
    echo "zstd source-adjustment directory does not exist: $source_directory" >&2
    return 1
  }
  [[ -f "$adjustment_file" ]] || {
    echo "zstd source-adjustment patch does not exist: $adjustment_file" >&2
    return 1
  }
  [[ -f "$hash_file" ]] || {
    echo "zstd source-adjustment hash list does not exist: $hash_file" >&2
    return 1
  }

  actual_adjustment_sha256=$(zstd_sha256_file "$adjustment_file")
  [[ "$actual_adjustment_sha256" == "$expected_adjustment_sha256" ]] || {
    echo "zstd source-adjustment patch SHA-256 mismatch" >&2
    return 1
  }
  actual_hash_file_sha256=$(zstd_sha256_file "$hash_file")
  [[ "$actual_hash_file_sha256" == "$expected_hash_file_sha256" ]] || {
    echo "zstd source-adjustment hash-list SHA-256 mismatch" >&2
    return 1
  }

  while read -r target before after; do
    [[ -n "$target" && "${target:0:1}" != "#" ]] || continue
    [[ -f "$source_directory/$target" ]] || {
      echo "zstd source-adjustment target does not exist: $target" >&2
      return 1
    }
    actual=$(zstd_sha256_file "$source_directory/$target")
    [[ "$actual" == "$before" ]] || {
      echo "zstd source-adjustment target preimage SHA-256 mismatch: $target" >&2
      return 1
    }
  done <"$hash_file"

  mkdir -p "$artifact_directory"
  cp "$adjustment_file" "$artifact_directory/source-adjustment.patch"
  cp "$hash_file" "$artifact_directory/source-adjustment-hashes.txt"

  if ! patch --dry-run --batch --forward --fuzz=0 --strip=1 \
    --directory="$source_directory" --input="$adjustment_file" \
    >"$application_log" 2>&1; then
    cat "$application_log" >&2
    echo "zstd source-adjustment patch does not apply exactly" >&2
    return 1
  fi
  if grep -Eq 'offset -?[0-9]|with fuzz' "$application_log"; then
    cat "$application_log" >&2
    echo "zstd source-adjustment patch requires a non-exact hunk match" >&2
    return 1
  fi
  if ! patch --batch --forward --fuzz=0 --strip=1 \
    --directory="$source_directory" --input="$adjustment_file" \
    >>"$application_log" 2>&1; then
    cat "$application_log" >&2
    echo "zstd source-adjustment patch failed after a successful dry run" >&2
    return 1
  fi

  while read -r target before after; do
    [[ -n "$target" && "${target:0:1}" != "#" ]] || continue
    actual=$(zstd_sha256_file "$source_directory/$target")
    [[ "$actual" == "$after" ]] || {
      echo "zstd source-adjustment target postimage SHA-256 mismatch: $target" >&2
      return 1
    }
  done <"$hash_file"

  {
    printf 'patch_sha256=%s\n' "$actual_adjustment_sha256"
    printf 'hash_list_sha256=%s\n' "$actual_hash_file_sha256"
    printf 'rationale=%s\n' "$rationale"
  } >"$artifact_directory/source-adjustment.txt"
}
