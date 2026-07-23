#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--source-archive PATH"* ]]
[[ "$help_output" == *"--work-dir PATH"* ]]
[[ "$help_output" == *"--jobs COUNT"* ]]

set +e
invalid_jobs_output=$("$script_directory/run.sh" --jobs 0 2>&1)
invalid_jobs_status=$?
unknown_output=$("$script_directory/run.sh" --unknown 2>&1)
unknown_status=$?
set -e

[[ "$invalid_jobs_status" == 2 ]]
[[ "$invalid_jobs_output" == *"positive integer"* ]]
[[ "$unknown_status" == 2 ]]
[[ "$unknown_output" == *"usage:"* ]]

"$script_directory/test-ccc-cc.sh"

echo "zstd runner tests passed"
