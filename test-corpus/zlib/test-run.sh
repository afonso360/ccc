#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--source-archive PATH"* ]]
[[ "$help_output" == *"--work-dir PATH"* ]]
[[ "$help_output" == *"--jobs COUNT"* ]]

set +e
missing_value_output=$("$script_directory/run.sh" --source-archive 2>&1)
missing_value_status=$?
set -e
[[ "$missing_value_status" == 2 ]]
[[ "$missing_value_output" == usage:* ]]

set +e
bad_jobs_output=$("$script_directory/run.sh" --jobs 0 2>&1)
bad_jobs_status=$?
set -e
[[ "$bad_jobs_status" == 2 ]]
[[ "$bad_jobs_output" == *"positive integer"* ]]

set +e
unknown_output=$("$script_directory/run.sh" --unknown 2>&1)
unknown_status=$?
set -e
[[ "$unknown_status" == 2 ]]
[[ "$unknown_output" == usage:* ]]

grep -Fq 'make -j"$jobs" test test64' "$script_directory/run.sh"
grep -Fq 'build_command = "make test test64"' "$script_directory/manifest.toml"
grep -Fq 'source_adjustments = "none"' "$script_directory/manifest.toml"
grep -Fq 'compiler_wrapper = "none"' "$script_directory/manifest.toml"

echo "zlib runner argument tests passed"
