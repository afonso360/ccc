#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--archive PATH"* ]]
[[ "$help_output" == *"--work-dir PATH"* ]]
[[ "$help_output" == *"--jobs COUNT"* ]]

set +e
invalid_jobs_output=$("$script_directory/run.sh" --jobs 0 2>&1)
invalid_jobs_status=$?
missing_value_output=$("$script_directory/run.sh" --archive 2>&1)
missing_value_status=$?
unknown_option_output=$("$script_directory/run.sh" --unknown 2>&1)
unknown_option_status=$?
set -e

[[ "$invalid_jobs_status" == 2 ]]
[[ "$invalid_jobs_output" == *"must be a positive integer"* ]]
[[ "$missing_value_status" == 2 ]]
[[ "$missing_value_output" == *"usage:"* ]]
[[ "$unknown_option_status" == 2 ]]
[[ "$unknown_option_output" == *"usage:"* ]]

"$script_directory/test-ccc-cc.sh"
"$script_directory/test-hosted-assert.sh"
"$script_directory/test-source-adjustment.sh"

echo "Redis runner tests passed"
