#!/usr/bin/env bash
set -euo pipefail
script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
temporary_directory=$(mktemp -d /tmp/ccc-real-world-benchmark-test.XXXXXX)
trap 'rm -rf -- "$temporary_directory"' EXIT

fake_program="$temporary_directory/fake-program"
cat >"$fake_program" <<'SH'
#!/usr/bin/env bash
set -eo pipefail
if [[ -n "$FAKE_PROGRAM_EXIT" && "$FAKE_PROGRAM_EXIT" != 0 ]]; then
  exit "$FAKE_PROGRAM_EXIT"
fi
for input in "$@"; do :; done
case "$input" in *.lua) exit 0 ;; esac
if [[ "$FAKE_CORRUPT_OUTPUT" == 1 ]]; then printf corrupt; else cat "$input"; fi
SH
chmod +x "$fake_program"
fixture="$temporary_directory/preexisting-compressed.fixture"
printf fixture-data >"$fixture"

results="$temporary_directory/results"
"$script_directory/run.py" --output "$results" \
  --program bzip2="$fake_program" --program zlib="$fake_program" \
  --program zstd="$fake_program" --program lua="$fake_program" \
  --decompression-input bzip2="$fixture" --decompression-input zlib="$fixture" \
  --decompression-input zstd="$fixture" --input-mebibytes 1 --warmups 0 --samples 2
[[ "$(wc -l <"$results/summary.tsv" | tr -d '[:space:]')" == 8 ]]
[[ "$(wc -l <"$results/run-times.tsv" | tr -d '[:space:]')" == 15 ]]
grep -Fq $'zstd\tdictionary-compression\tdecompression\tinput-byte\t1048576\t1048576\t0\t2' "$results/summary.tsv"
grep -Fq $'bzip2\tdecompression-fixture\t'"$fixture"$'\t12\t' "$results/artifacts.tsv"
grep -Fq '"format_version":2' "$results/environment.json"
if rg -n 'validate|validation|validation_sha256|digest' "$results/commands.jsonl" "$results/artifacts.tsv" "$results/summary.tsv"; then
  echo "timing results unexpectedly contain validation evidence" >&2; exit 1
fi
[[ -z "$(find "$results" -name '*validation*' -print -quit)" ]]
if grep -Fq 'checksum' "$results/inputs/interpreter-workload.lua" || grep -Fq 'error(' "$results/inputs/interpreter-workload.lua"; then
  echo "timing Lua workload contains a correctness oracle" >&2; exit 1
fi

corrupt_timing_results="$temporary_directory/corrupt-timing"
FAKE_CORRUPT_OUTPUT=1 "$script_directory/run.py" --output "$corrupt_timing_results" \
  --cases bzip2 --operations compression,decompression --program bzip2="$fake_program" \
  --decompression-input bzip2="$fixture" --input-mebibytes 1 --warmups 0 --samples 1
[[ -s "$corrupt_timing_results/summary.tsv" ]]

missing_fixture_results="$temporary_directory/missing-fixture"
set +e
missing_fixture_output=$("$script_directory/run.py" --output "$missing_fixture_results" \
  --cases bzip2 --operations decompression --program bzip2="$fake_program" --samples 1 2>&1)
missing_fixture_status=$?
set -e
[[ "$missing_fixture_status" == 1 ]]
[[ "$missing_fixture_output" == *"--decompression-input is required"* ]]

validation_results="$temporary_directory/validation"
"$script_directory/validate.py" --output "$validation_results" \
  --program bzip2="$fake_program" --program zlib="$fake_program" \
  --program zstd="$fake_program" --program lua="$fake_program" --input-mebibytes 1
[[ ! -e "$validation_results/summary.tsv" ]]
grep -Fq '"kind":"validate-compression"' "$validation_results/commands.jsonl"
grep -Fq $'bzip2\tvalidation-decompressed\t' "$validation_results/artifacts.tsv"
grep -Fq '"operation":"round-trip"' "$validation_results/validation.json"
grep -Fq 'error("Lua workload checksum mismatch")' "$validation_results/inputs/interpreter-workload.lua"

failing_validation="$temporary_directory/failing-validation"
set +e
validation_failure_output=$(FAKE_CORRUPT_OUTPUT=1 "$script_directory/validate.py" \
  --output "$failing_validation" --cases bzip2 --program bzip2="$fake_program" --input-mebibytes 1 2>&1)
validation_failure_status=$?
set -e
[[ "$validation_failure_status" == 1 ]]
[[ "$validation_failure_output" == *"decompression produced"* ]]

help_output=$("$script_directory/run.py" --help)
[[ "$help_output" == *"--operations"* && "$help_output" == *"--decompression-input"* ]]
[[ "$help_output" == *"never validates program correctness"* ]]
validate_help=$("$script_directory/validate.py" --help)
[[ "$validate_help" == *"without benchmark summaries"* ]]
