#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-c-ray-runner-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--profile NAME"* ]]
[[ "$help_output" == *"--source-archive PATH"* ]]
[[ "$help_output" == *"--warmups COUNT"* ]]
[[ "$help_output" == *"--samples COUNT"* ]]
[[ "$help_output" == *"--reference-cc PATH"* ]]

set +e
missing_value_output=$("$script_directory/run.sh" --profile 2>&1)
missing_value_status=$?
set -e
[[ "$missing_value_status" == 2 ]]
[[ "$missing_value_output" == *"missing value for --profile"* ]]

set +e
bad_profile_output=$("$script_directory/run.sh" --profile unknown 2>&1)
bad_profile_status=$?
set -e
[[ "$bad_profile_status" == 2 ]]
[[ "$bad_profile_output" == *"unsupported C-Ray profile"* ]]

set +e
bad_warmups_output=$("$script_directory/run.sh" --warmups -1 2>&1)
bad_warmups_status=$?
set -e
[[ "$bad_warmups_status" == 2 ]]
[[ "$bad_warmups_output" == *"--warmups must be a nonnegative integer"* ]]

set +e
bad_samples_output=$("$script_directory/run.sh" --samples 0 2>&1)
bad_samples_status=$?
set -e
[[ "$bad_samples_status" == 2 ]]
[[ "$bad_samples_output" == *"--samples must be a positive integer"* ]]

grep -Fq \
  'archive_sha256 = "6f507aae47a9367334b8cb50f50eb4ad0f6fef99aeae9f2f7d55ba9818e798bf"' \
  "$script_directory/manifest.toml"
grep -Fq \
  'source_sha256 = "4847631a6df9af1685576d62c71975619dd280a70aa40670f7d0ca00bf4eb63a"' \
  "$script_directory/manifest.toml"
grep -Fq \
  'correctness_scene_sha256 = "a50d2d8effe6b2372830780db80131bd655708a9b645ad6c895d16590cce4515"' \
  "$script_directory/manifest.toml"
grep -Fq \
  'performance_scene_sha256 = "b84ee97d101ebf6eed605ba9b5938019ca0651c128b8b4c8053fdcdd7a7c1741"' \
  "$script_directory/manifest.toml"
grep -Fq 'source_adjustments = "none"' "$script_directory/manifest.toml"
for retained in \
  codegen-stats.tsv object-section-totals.tsv object-sections.tsv \
  object-sections.txt; do
  grep -Fq "\"$retained\"" "$script_directory/manifest.toml"
done

valid_ppm="$temporary_directory/valid.ppm"
comparison_ppm="$temporary_directory/comparison.ppm"
invalid_ppm="$temporary_directory/invalid.ppm"
printf 'P6\n2 1\n255\nabcdef' >"$valid_ppm"
cp "$valid_ppm" "$comparison_ppm"
printf 'P6\n2 1\n255\nabcde' >"$invalid_ppm"
valid_hash=$(
  "$script_directory/validate-ppm.py" \
    --path "$valid_ppm" --width 2 --height 1 --compare "$comparison_ppm"
)
[[ "$valid_hash" =~ ^[0-9a-f]{64}$ ]]
if "$script_directory/validate-ppm.py" \
  --path "$invalid_ppm" --width 2 --height 1 >/dev/null 2>&1; then
  echo "truncated PPM unexpectedly passed validation" >&2
  exit 1
fi
printf 'P6\n2 1\n255\nabcdeg' >"$comparison_ppm"
if "$script_directory/validate-ppm.py" \
  --path "$valid_ppm" --width 2 --height 1 \
  --compare "$comparison_ppm" >/dev/null 2>&1; then
  echo "different PPM unexpectedly passed comparison" >&2
  exit 1
fi

timings="$temporary_directory/timings.tsv"
timing_json="$temporary_directory/timing.json"
measured_stdout="$temporary_directory/measured.stdout"
measured_stderr="$temporary_directory/measured.stderr"
"$script_directory/measure-command.py" \
  --stage render-sample \
  --label fixture \
  --iteration 1 \
  --json "$timing_json" \
  --results "$timings" \
  --stdout "$measured_stdout" \
  --stderr "$measured_stderr" \
  -- /bin/sh -c 'printf subject-output; printf subject-error >&2'
[[ "$(cat "$measured_stdout")" == subject-output ]]
[[ "$(cat "$measured_stderr")" == subject-error ]]
grep -Fq '"exit_status":0' "$timing_json"
grep -Fq $'stage\tlabel\titeration\twall_seconds' "$timings"
grep -Fq $'render-sample\tfixture\t1\t' "$timings"

fake_size="$temporary_directory/fake-size"
cat >"$fake_size" <<'EOF'
#!/bin/sh
if [ "$1" != "--format=sysv" ]; then
  echo "expected --format=sysv" >&2
  exit 2
fi
case "$2" in
  fixture-darwin.o)
    cat <<'OUTPUT'
fixture-darwin.o  :
section        size   addr
__text          100      0
__const          20    100
__const           5    120
__eh_frame        8    125
__data            4    133
__bss            16    137
__stubs           3    153
Total           156
OUTPUT
    ;;
  fixture-elf.o)
    cat <<'OUTPUT'
fixture-elf.o  :
section          size   addr
.text              80      0
.text.hot          10     80
.rodata            12     90
.data               2    102
.bss                6    104
.debug_info          9    110
.eh_frame            7    119
.comment             1    126
Total              127
OUTPUT
    ;;
  *)
    echo "unexpected artifact: $2" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$fake_size"
: >"$temporary_directory/fixture-darwin.o"
: >"$temporary_directory/fixture-elf.o"
"$script_directory/collect-object-sections.py" \
  --size-tool "$fake_size" \
  --sections-output "$temporary_directory/object-sections.tsv" \
  --totals-output "$temporary_directory/object-section-totals.tsv" \
  --raw-output "$temporary_directory/object-sections.txt" \
  --artifact fixture-darwin "$temporary_directory/fixture-darwin.o" \
  --artifact fixture-elf "$temporary_directory/fixture-elf.o"
grep -Fq $'fixture-darwin\t__const\tread_only_data\t25' \
  "$temporary_directory/object-sections.tsv"
grep -Fq $'fixture-darwin\t100\t25\t4\t16\t8\t0\t3\t156' \
  "$temporary_directory/object-section-totals.tsv"
grep -Fq $'fixture-elf\t90\t12\t2\t6\t7\t9\t1\t127' \
  "$temporary_directory/object-section-totals.tsv"
grep -Fq '== fixture-darwin ==' \
  "$temporary_directory/object-sections.txt"

cat >"$temporary_directory/summary-timings.tsv" <<'EOF'
stage	label	iteration	wall_seconds	user_seconds	system_seconds	peak_rss_bytes	exit_status
compile	fixture	0	0.500000000	0.4	0.1	1024	0
link	fixture	0	0.250000000	0.2	0.05	2048	0
render-sample	fixture	1	3.000000000	2.9	0.1	3072	0
render-sample	fixture	2	1.000000000	0.9	0.1	4096	0
render-sample	fixture	3	2.000000000	1.9	0.1	5120	0
EOF
cat >"$temporary_directory/artifacts.tsv" <<'EOF'
label	object_bytes	executable_bytes
fixture	123	456
EOF
cat >"$temporary_directory/codegen-stats.tsv" <<'EOF'
label	metric	value
fixture	schema_version	2
fixture	post_inline_ir.functions	2
fixture	post_inline_ir.blocks	7
fixture	post_inline_ir.values	31
fixture	post_inline_ir.instructions	40
fixture	post_inline_ir.call_instructions	3
fixture	post_inline_ir.fixed_stack_slots	4
fixture	post_inline_ir.fixed_stack_bytes	64
fixture	post_inline_ir.dynamic_stack_slots	0
fixture	post_inline_ir.signatures	3
fixture	post_inline_ir.external_functions	3
fixture	post_inline_ir.global_values	2
EOF
cat >"$temporary_directory/summary-object-sections.tsv" <<'EOF'
label	text_bytes	read_only_data_bytes	writable_data_bytes	bss_bytes	unwind_bytes	debug_bytes	other_bytes	total_section_bytes
fixture	100	25	4	16	8	0	3	156
EOF
cat >"$temporary_directory/hashes.tsv" <<'EOF'
label	phase	iteration	sha256
fixture	check	0	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
fixture	sample	1	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
EOF
"$script_directory/summarize.py" \
  --timings "$temporary_directory/summary-timings.tsv" \
  --artifacts "$temporary_directory/artifacts.tsv" \
  --codegen-stats "$temporary_directory/codegen-stats.tsv" \
  --object-sections "$temporary_directory/summary-object-sections.tsv" \
  --hashes "$temporary_directory/hashes.tsv" \
  --output "$temporary_directory/summary.tsv"
grep -Fq $'fixture\t0.500000000\t0.250000000\t3\t2.000000000\t1.000000000\t3.000000000' \
  "$temporary_directory/summary.tsv"
grep -Fq $'\t2\t7\t31\t40\t3\t4\t64\t0\t3\t3\t2\t123\t100\t25\t4\t16\t8\t0\t3\t156\t456\t' \
  "$temporary_directory/summary.tsv"

bash -n "$script_directory/run.sh"

echo "C-Ray runner and result-tool regressions passed"
