#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
temporary_directory=$(mktemp -d /tmp/ccc-real-world-benchmark-test.XXXXXX)
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

fake_program="$temporary_directory/fake-program"
cat >"$fake_program" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$FAKE_PROGRAM_EXIT" != 0 ]]; then
  exit "$FAKE_PROGRAM_EXIT"
fi
for input; do
  :
done
case "$input" in
  *.lua) exit 0 ;;
esac
cat "$input"
SH
chmod +x "$fake_program"

results="$temporary_directory/results"
FAKE_PROGRAM_EXIT=0 "$script_directory/run.py" \
  --output "$results" \
  --program bzip2="$fake_program" \
  --program zlib="$fake_program" \
  --program zstd="$fake_program" \
  --program lua="$fake_program" \
  --input-mebibytes 1 \
  --warmups 0 \
  --samples 2

[[ "$(wc -l <"$results/summary.tsv" | tr -d '[:space:]')" == 8 ]]
[[ "$(wc -l <"$results/run-times.tsv" | tr -d '[:space:]')" == 15 ]]
[[ ! -e "$results/build-times.tsv" ]]
grep -Fq $'bzip2\tblock-sorting-compression\tcompression\tinput-byte\t1048576\t1048576\t0\t2' \
  "$results/summary.tsv"
grep -Fq $'zstd\tdictionary-compression\tdecompression\tinput-byte\t1048576\t1048576\t0\t2' \
  "$results/summary.tsv"
grep -Fq $'lua\tbytecode-interpreter\tinterpreter\tinterpreter-step\t3145728\t0\t0\t2' \
  "$results/summary.tsv"
grep -Fq $'shared\tcompression-input\tinputs/mixed-input.bin\t1048576\t' \
  "$results/artifacts.tsv"
grep -Fq $'bzip2\tvalidation-decompressed\t' "$results/artifacts.tsv"
grep -Fq '"format_version":1' "$results/environment.json"
grep -Fq '"kind":"validate-compression"' "$results/commands.jsonl"
grep -Fq '"kind":"interpreter"' "$results/commands.jsonl"

failing_results="$temporary_directory/failing-results"
set +e
failure_output=$(
  FAKE_PROGRAM_EXIT=7 "$script_directory/run.py" \
    --output "$failing_results" \
    --cases bzip2 \
    --program bzip2="$fake_program" \
    --input-mebibytes 1 \
    --warmups 0 \
    --samples 1 2>&1
)
failure_status=$?
set -e
[[ "$failure_status" == 1 ]]
[[ "$failure_output" == *"validation compression failed with status 7"* ]]

help_output=$("$script_directory/run.py" --help)
[[ "$help_output" == *"--program"* ]]
[[ "$help_output" == *"never builds or tests"* ]]
