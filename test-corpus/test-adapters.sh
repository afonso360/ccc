#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

tests=(
  "$script_directory/test-adapter-environment.sh"
  "$script_directory/test-report-target-applicability.sh"
  "$script_directory/report-target-applicability.py"
  "$script_directory/sqlite/test-ccc-cc.sh"
  "$script_directory/sqlite/test-run.sh"
  "$script_directory/lua/test-ccc-cc.sh"
  "$script_directory/bzip2/test-ccc-cc.sh"
  "$script_directory/bzip2/test-run.sh"
  "$script_directory/redis/test-ccc-cc.sh"
  "$script_directory/redis/test-run.sh"
  "$script_directory/zstd/test-ccc-cc.sh"
  "$script_directory/zstd/test-run.sh"
)

for test_script in "${tests[@]}"; do
  [[ -x "$test_script" ]] || {
    echo "corpus adapter regression is not executable: $test_script" >&2
    exit 1
  }
  printf 'running %s\n' "${test_script#"$script_directory/"}"
  "$test_script"
done

echo "all corpus adapter regressions passed"
