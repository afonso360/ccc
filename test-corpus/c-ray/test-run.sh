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
grep -Fq 'format_version = 3' "$script_directory/manifest.toml"
grep -Fq 'source_adjustments = "none"' "$script_directory/manifest.toml"
for retained in \
  codegen-stats.tsv compile-phase-artifacts.tsv compile-phase-timings.tsv \
  object-section-totals.tsv object-sections.tsv object-sections.txt \
  phase-timing-raw; do
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

phase_fixture_directory="$temporary_directory/phase-fixture"
mkdir -p "$phase_fixture_directory"
printf 'stable measured object\n' >"$phase_fixture_directory/canonical.o"
fake_phase_compiler="$phase_fixture_directory/fake-phase-compiler"
cat >"$fake_phase_compiler" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mode=$1
object=$2
sidecar=$3
canonical=$4
printf 'phase compiler stdout\n'
printf 'phase compiler stderr\n' >&2
if [[ "$mode" == mismatch ]]; then
  printf 'different instrumented object\n' >"$object"
else
  cp "$canonical" "$object"
fi
case "$mode" in
  valid | mismatch)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	13
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
    ;;
  missing-sidecar)
    ;;
  missing-metric)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	13
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
pipeline	37
TIMINGS
    ;;
  malformed)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	013
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
    ;;
  duplicate)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	13
parsing	14
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
    ;;
  reordered)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
parsing	13
preprocessing	11
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
    ;;
  unexpected)
    cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	13
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
frontend.unexpected	30
object_packaging	31
pipeline	37
TIMINGS
    ;;
  wrong-version)
    cat >"$sidecar" <<'TIMINGS'
schema_version	2
preprocessing	11
parsing	13
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
    ;;
  *)
    echo "unknown phase fixture mode: $mode" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$fake_phase_compiler"

run_phase_collector() {
  local mode=$1
  local label=$2
  local raw_directory=$3
  local phase_results=$4
  local artifact_results=$5
  mkdir -p "$raw_directory"
  "$script_directory/collect-phase-timings.py" \
    --label "$label" \
    --canonical-object "$phase_fixture_directory/canonical.o" \
    --instrumented-object "$raw_directory/phase-timings.o" \
    --phase-sidecar "$raw_directory/phase-timings.tsv" \
    --stdout "$raw_directory/phase-timings.stdout.txt" \
    --stderr "$raw_directory/phase-timings.stderr.txt" \
    --command-output "$raw_directory/phase-timings.command.txt" \
    --result-output "$raw_directory/phase-timings.result.json" \
    --phase-results "$phase_results" \
    --artifact-results "$artifact_results" \
    -- "$fake_phase_compiler" "$mode" \
    "$raw_directory/phase-timings.o" \
    "$raw_directory/phase-timings.tsv" \
    "$phase_fixture_directory/canonical.o"
}

phase_results="$phase_fixture_directory/compile-phase-timings.tsv"
phase_artifact_results="$phase_fixture_directory/compile-phase-artifacts.tsv"
cp "$timings" "$phase_fixture_directory/measured-timings.before.tsv"
run_phase_collector \
  valid ccc-o0 "$phase_fixture_directory/raw-valid" \
  "$phase_results" "$phase_artifact_results"
cmp "$timings" "$phase_fixture_directory/measured-timings.before.tsv"
[[ "$(wc -l <"$phase_results" | tr -d '[:space:]')" == 10 ]]
grep -Fq $'ccc-o0\tcodegen.total\t29' "$phase_results"
grep -Eq $'^ccc-o0\t[0-9]+\t[0-9a-f]{64}\t[0-9]+\t[0-9a-f]{64}\t1$' \
  "$phase_artifact_results"
grep -Fq 'LC_ALL=C ' \
  "$phase_fixture_directory/raw-valid/phase-timings.command.txt"
grep -Fq '"format_version":3' \
  "$phase_fixture_directory/raw-valid/phase-timings.result.json"
grep -Fq '"timed":false' \
  "$phase_fixture_directory/raw-valid/phase-timings.result.json"
[[ "$(cat "$phase_fixture_directory/raw-valid/phase-timings.stdout.txt")" == \
  "phase compiler stdout" ]]
[[ "$(cat "$phase_fixture_directory/raw-valid/phase-timings.stderr.txt")" == \
  "phase compiler stderr" ]]

expect_phase_collection_failure() {
  local mode=$1
  local expected=$2
  local case_directory="$phase_fixture_directory/failure-$mode"
  local output status
  mkdir -p "$case_directory"
  cp "$phase_results" "$case_directory/phases.tsv"
  cp "$phase_artifact_results" "$case_directory/artifacts.tsv"
  cp "$case_directory/phases.tsv" "$case_directory/phases.before.tsv"
  cp "$case_directory/artifacts.tsv" "$case_directory/artifacts.before.tsv"
  set +e
  output=$(
    run_phase_collector \
      "$mode" ccc-o2 "$case_directory/raw" \
      "$case_directory/phases.tsv" "$case_directory/artifacts.tsv" 2>&1
  )
  status=$?
  set -e
  [[ "$status" == 1 ]]
  [[ "$output" == *"$expected"* ]]
  grep -Fq '"validation_error":' \
    "$case_directory/raw/phase-timings.result.json"
  cmp "$case_directory/phases.tsv" "$case_directory/phases.before.tsv"
  cmp "$case_directory/artifacts.tsv" "$case_directory/artifacts.before.tsv"
}

expect_phase_collection_failure \
  mismatch "phase-timing instrumentation changed the measured object"
expect_phase_collection_failure \
  missing-metric "missing object_packaging"
expect_phase_collection_failure \
  missing-sidecar "phase-timing compile did not create"
expect_phase_collection_failure \
  malformed "is not canonical unsigned decimal"
expect_phase_collection_failure \
  duplicate "duplicate phase-timing metric 'parsing'"
expect_phase_collection_failure \
  reordered "metrics are out of schema order"
expect_phase_collection_failure \
  unexpected "unexpected frontend.unexpected"
expect_phase_collection_failure \
  wrong-version "unsupported phase-timing schema 2"

set +e
cp "$phase_results" "$phase_fixture_directory/phases.before-duplicate.tsv"
cp \
  "$phase_artifact_results" \
  "$phase_fixture_directory/artifacts.before-duplicate.tsv"
duplicate_record_output=$(
  run_phase_collector \
    valid ccc-o0 "$phase_fixture_directory/raw-duplicate-record" \
    "$phase_results" "$phase_artifact_results" 2>&1
)
duplicate_record_status=$?
set -e
[[ "$duplicate_record_status" == 1 ]]
[[ "$duplicate_record_output" == *"duplicate phase-timing record for ccc-o0"* ]]
[[ ! -e "$phase_fixture_directory/raw-duplicate-record/phase-timings.o" ]]
cmp "$phase_results" "$phase_fixture_directory/phases.before-duplicate.tsv"
cmp \
  "$phase_artifact_results" \
  "$phase_fixture_directory/artifacts.before-duplicate.tsv"

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
compile	ccc-o0	0	0.500000000	0.4	0.1	1024	0
link	ccc-o0	0	0.250000000	0.2	0.05	2048	0
render-sample	ccc-o0	1	3.000000000	2.9	0.1	3072	0
render-sample	ccc-o0	2	1.000000000	0.9	0.1	4096	0
render-sample	ccc-o0	3	2.000000000	1.9	0.1	5120	0
compile	ccc-o2	0	0.400000000	0.3	0.1	1024	0
link	ccc-o2	0	0.200000000	0.1	0.1	2048	0
render-sample	ccc-o2	1	2.000000000	1.9	0.1	3072	0
compile	ccc-oz	0	0.450000000	0.35	0.1	1024	0
link	ccc-oz	0	0.225000000	0.125	0.1	2048	0
render-sample	ccc-oz	1	2.250000000	2.15	0.1	3072	0
compile	reference-o2	0	0.600000000	0.5	0.1	1024	0
link	reference-o2	0	0.300000000	0.2	0.1	2048	0
render-sample	reference-o2	1	2.500000000	2.4	0.1	3072	0
EOF
cat >"$temporary_directory/artifacts.tsv" <<'EOF'
label	object_bytes	executable_bytes
ccc-o0	123	456
ccc-o2	123	456
ccc-oz	123	456
reference-o2	321	654
EOF
cat >"$temporary_directory/codegen-stats.tsv" <<'EOF'
label	metric	value
ccc-o0	schema_version	3
ccc-o0	post_inline_ir.functions	2
ccc-o0	post_inline_ir.blocks	7
ccc-o0	post_inline_ir.values	31
ccc-o0	post_inline_ir.instructions	40
ccc-o0	post_inline_ir.call_instructions	3
ccc-o0	post_inline_ir.fixed_stack_slots	4
ccc-o0	post_inline_ir.fixed_stack_bytes	64
ccc-o0	post_inline_ir.dynamic_stack_slots	0
ccc-o0	post_inline_ir.signatures	3
ccc-o0	post_inline_ir.unused_signatures	1
ccc-o0	post_inline_ir.external_functions	3
ccc-o0	post_inline_ir.unused_external_functions	1
ccc-o0	post_inline_ir.global_values	2
ccc-o0	post_inline_ir.unused_global_values	0
ccc-o0	post_inline_ir.constants	0
ccc-o0	post_inline_ir.jump_tables	0
ccc-o0	primary_object.file_bytes	123
ccc-o0	primary_object.sections	7
ccc-o0	primary_object.symbols	4
ccc-o0	primary_object.defined_symbols	2
ccc-o0	primary_object.undefined_symbols	2
ccc-o0	primary_object.relocations	3
ccc-o0	primary_object.text_bytes	100
ccc-o0	primary_object.read_only_data_bytes	25
ccc-o0	primary_object.writable_data_bytes	4
ccc-o0	primary_object.bss_bytes	16
ccc-o0	primary_object.tls_data_bytes	0
ccc-o0	primary_object.tls_bss_bytes	0
ccc-o0	primary_object.unwind_bytes	8
ccc-o0	primary_object.debug_bytes	0
ccc-o0	primary_object.metadata_bytes	3
ccc-o0	primary_object.other_section_bytes	0
EOF
cp \
  "$temporary_directory/codegen-stats.tsv" \
  "$temporary_directory/codegen-stats-ccc-o0.tsv"
for label in ccc-o2 ccc-oz; do
  sed \
    -e '1d' \
    -e "s/^ccc-o0/$label/" \
    "$temporary_directory/codegen-stats-ccc-o0.tsv" \
    >>"$temporary_directory/codegen-stats.tsv"
done
cat >"$temporary_directory/summary-object-sections.tsv" <<'EOF'
label	text_bytes	read_only_data_bytes	writable_data_bytes	bss_bytes	unwind_bytes	debug_bytes	other_bytes	total_section_bytes
ccc-o0	100	25	4	16	8	0	3	156
ccc-o2	100	25	4	16	8	0	3	156
ccc-oz	100	25	4	16	8	0	3	156
reference-o2	200	20	8	10	9	0	4	251
EOF
cat >"$temporary_directory/hashes.tsv" <<'EOF'
label	phase	iteration	sha256
ccc-o0	check	0	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
ccc-o0	sample	1	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
ccc-o2	check	0	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
ccc-o2	sample	1	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
ccc-oz	check	0	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
ccc-oz	sample	1	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
reference-o2	check	0	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
reference-o2	sample	1	aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
EOF
cat >"$temporary_directory/summary-phase-timings.tsv" <<'EOF'
label	metric	value
ccc-o0	schema_version	1
ccc-o0	preprocessing	11
ccc-o0	parsing	13
ccc-o0	semantic_analysis	17
ccc-o0	ccc_ir_lowering	19
ccc-o0	ccc_ir_optimization	23
ccc-o0	codegen.total	29
ccc-o0	object_packaging	31
ccc-o0	pipeline	37
ccc-o2	schema_version	1
ccc-o2	preprocessing	11
ccc-o2	parsing	13
ccc-o2	semantic_analysis	17
ccc-o2	ccc_ir_lowering	19
ccc-o2	ccc_ir_optimization	23
ccc-o2	codegen.total	29
ccc-o2	object_packaging	31
ccc-o2	pipeline	37
ccc-oz	schema_version	1
ccc-oz	preprocessing	11
ccc-oz	parsing	13
ccc-oz	semantic_analysis	17
ccc-oz	ccc_ir_lowering	19
ccc-oz	ccc_ir_optimization	23
ccc-oz	codegen.total	29
ccc-oz	object_packaging	31
ccc-oz	pipeline	37
EOF
cat >"$temporary_directory/summary-phase-artifacts.tsv" <<'EOF'
label	canonical_object_bytes	canonical_object_sha256	instrumented_object_bytes	instrumented_object_sha256	objects_match
ccc-o0	123	bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb	123	bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb	1
ccc-o2	123	cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc	123	cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc	1
ccc-oz	123	dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd	123	dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd	1
EOF
"$script_directory/summarize.py" \
  --timings "$temporary_directory/summary-timings.tsv" \
  --artifacts "$temporary_directory/artifacts.tsv" \
  --codegen-stats "$temporary_directory/codegen-stats.tsv" \
  --phase-timings "$temporary_directory/summary-phase-timings.tsv" \
  --phase-artifacts "$temporary_directory/summary-phase-artifacts.tsv" \
  --object-sections "$temporary_directory/summary-object-sections.tsv" \
  --hashes "$temporary_directory/hashes.tsv" \
  --output "$temporary_directory/summary.tsv"
grep -Fq $'ccc-o0\t0.500000000\t0.250000000\t3\t2.000000000\t1.000000000\t3.000000000' \
  "$temporary_directory/summary.tsv"
grep -Fq $'\t11\t13\t17\t19\t23\t29\t31\t37\t2\t7\t31\t40\t3\t4\t64\t0\t3\t1\t3\t1\t2\t0\t123\t100\t25\t4\t16\t8\t0\t3\t156\t456\t' \
  "$temporary_directory/summary.tsv"
python3 - "$temporary_directory/summary.tsv" <<'PY'
import csv
import sys

with open(sys.argv[1], encoding="utf-8", newline="") as source:
    rows = {row["label"]: row for row in csv.DictReader(source, delimiter="\t")}
phase_fields = [
    field for field in rows["ccc-o0"] if field.startswith("compile_phase_")
]
assert len(phase_fields) == 8
assert all(rows["ccc-o0"][field] for field in phase_fields)
assert all(rows["reference-o2"][field] == "" for field in phase_fields)
PY

grep -Fv $'ccc-o0\tpost_inline_ir.unused_global_values\t0' \
  "$temporary_directory/codegen-stats.tsv" \
  >"$temporary_directory/incomplete-codegen-stats.tsv"
set +e
incomplete_stats_output=$(
  "$script_directory/summarize.py" \
    --timings "$temporary_directory/summary-timings.tsv" \
    --artifacts "$temporary_directory/artifacts.tsv" \
    --codegen-stats "$temporary_directory/incomplete-codegen-stats.tsv" \
    --phase-timings "$temporary_directory/summary-phase-timings.tsv" \
    --phase-artifacts "$temporary_directory/summary-phase-artifacts.tsv" \
    --object-sections "$temporary_directory/summary-object-sections.tsv" \
    --hashes "$temporary_directory/hashes.tsv" \
    --output "$temporary_directory/incomplete-summary.tsv" 2>&1
)
incomplete_stats_status=$?
set -e
[[ "$incomplete_stats_status" == 1 ]]
[[ "$incomplete_stats_output" == *"missing codegen statistics ['post_inline_ir.unused_global_values']"* ]]

printf '%s\n' \
  $'label\tcanonical_object_bytes\tcanonical_object_sha256\tinstrumented_object_bytes\tinstrumented_object_sha256\tobjects_match' \
  >"$temporary_directory/missing-phase-artifacts.tsv"
set +e
missing_phase_output=$(
  "$script_directory/summarize.py" \
    --timings "$temporary_directory/summary-timings.tsv" \
    --artifacts "$temporary_directory/artifacts.tsv" \
    --codegen-stats "$temporary_directory/codegen-stats.tsv" \
    --phase-timings "$temporary_directory/summary-phase-timings.tsv" \
    --phase-artifacts "$temporary_directory/missing-phase-artifacts.tsv" \
    --object-sections "$temporary_directory/summary-object-sections.tsv" \
    --hashes "$temporary_directory/hashes.tsv" \
    --output "$temporary_directory/missing-phase-summary.tsv" 2>&1
)
missing_phase_status=$?
set -e
[[ "$missing_phase_status" == 1 ]]
[[ "$missing_phase_output" == *"phase-artifact records do not match CCC artifacts"* ]]
[[ ! -e "$temporary_directory/missing-phase-summary.tsv" ]]

expect_artifact_label_failure() {
  local name=$1
  local artifacts=$2
  local expected=$3
  local output status
  set +e
  output=$(
    "$script_directory/summarize.py" \
      --timings "$temporary_directory/summary-timings.tsv" \
      --artifacts "$artifacts" \
      --codegen-stats "$temporary_directory/codegen-stats.tsv" \
      --phase-timings "$temporary_directory/summary-phase-timings.tsv" \
      --phase-artifacts "$temporary_directory/summary-phase-artifacts.tsv" \
      --object-sections "$temporary_directory/summary-object-sections.tsv" \
      --hashes "$temporary_directory/hashes.tsv" \
      --output "$temporary_directory/$name-summary.tsv" 2>&1
  )
  status=$?
  set -e
  [[ "$status" == 1 ]]
  [[ "$output" == *"$expected"* ]]
  [[ ! -e "$temporary_directory/$name-summary.tsv" ]]
}

grep -Fv $'ccc-oz\t' \
  "$temporary_directory/artifacts.tsv" \
  >"$temporary_directory/missing-label-artifacts.tsv"
expect_artifact_label_failure \
  missing-label "$temporary_directory/missing-label-artifacts.tsv" \
  "missing ['ccc-oz']"

cp "$temporary_directory/artifacts.tsv" \
  "$temporary_directory/extra-label-artifacts.tsv"
printf 'ccc-o3\t123\t456\n' \
  >>"$temporary_directory/extra-label-artifacts.tsv"
expect_artifact_label_failure \
  extra-label "$temporary_directory/extra-label-artifacts.tsv" \
  "unexpected ['ccc-o3']"

sed 's/^ccc-oz/ccc-os/' \
  "$temporary_directory/artifacts.tsv" \
  >"$temporary_directory/renamed-label-artifacts.tsv"
expect_artifact_label_failure \
  renamed-label "$temporary_directory/renamed-label-artifacts.tsv" \
  "missing ['ccc-oz']; unexpected ['ccc-os']"

{
  head -n 1 "$temporary_directory/artifacts.tsv"
  grep -F $'ccc-o2\t' "$temporary_directory/artifacts.tsv"
  grep -F $'ccc-o0\t' "$temporary_directory/artifacts.tsv"
  grep -F $'ccc-oz\t' "$temporary_directory/artifacts.tsv"
  grep -F $'reference-o2\t' "$temporary_directory/artifacts.tsv"
} >"$temporary_directory/reordered-label-artifacts.tsv"
expect_artifact_label_failure \
  reordered-label "$temporary_directory/reordered-label-artifacts.tsv" \
  "artifact labels are out of result-schema order"

fake_run_root="$temporary_directory/fake-run"
fake_adapter="$fake_run_root/adapter"
fake_tools="$fake_run_root/tools"
fake_resource_directory="$fake_run_root/resource-dir"
fake_sdk="$fake_run_root/sdk"
fake_source_parent="$fake_run_root/archive-source"
fake_source_directory="$fake_source_parent/c-ray-1.1"
mkdir -p \
  "$fake_adapter" "$fake_tools" "$fake_resource_directory" "$fake_sdk" \
  "$fake_source_directory"
fake_sdk=$(CDPATH='' cd -- "$fake_sdk" && pwd -P)
for adapter_file in \
  collect-object-sections.py collect-phase-timings.py manifest.toml \
  measure-command.py run.sh summarize.py validate-ppm.py; do
  cp "$script_directory/$adapter_file" "$fake_adapter/$adapter_file"
done
cat >"$fake_source_directory/c-ray-mt.c" <<'EOF'
/*
Copyright (C) 2006 John Tsiombikas
This fixture retains the upstream notice shape.
GNU General Public License v2 or (at your option) later
It exists only for the adapter's fake-run regression.
No benchmark result uses this source.
*/
int main(void) { return 0; }
EOF
printf 'fixture scene\n' >"$fake_source_directory/scene"
printf 'fixture scene\n' >"$fake_source_directory/sphfract"
fake_archive="$fake_run_root/c-ray-1.1.tar.gz"
tar -czf "$fake_archive" -C "$fake_source_parent" c-ray-1.1
fake_hash() {
  openssl dgst "-$1" "$2" | awk '{print $NF}'
}
fake_archive_bytes=$(wc -c <"$fake_archive" | tr -d '[:space:]')
fake_archive_sha256=$(fake_hash sha256 "$fake_archive")
fake_archive_sha3=$(fake_hash sha3-256 "$fake_archive")
fake_source_bytes=$(
  wc -c <"$fake_source_directory/c-ray-mt.c" | tr -d '[:space:]'
)
fake_source_sha256=$(fake_hash sha256 "$fake_source_directory/c-ray-mt.c")
fake_scene_bytes=$(wc -c <"$fake_source_directory/scene" | tr -d '[:space:]')
fake_scene_sha256=$(fake_hash sha256 "$fake_source_directory/scene")
cat >"$fake_adapter/manifest.toml" <<EOF
format_version = 3
name = "c-ray"
version = "1.1"
revision = "fake"
origin = "https://invalid.example/c-ray-1.1.tar.gz"
archive = "c-ray-1.1.tar.gz"
archive_bytes = $fake_archive_bytes
archive_sha256 = "$fake_archive_sha256"
archive_sha3_256 = "$fake_archive_sha3"
source = "c-ray-mt.c"
source_bytes = $fake_source_bytes
source_sha256 = "$fake_source_sha256"
correctness_scene = "scene"
correctness_scene_bytes = $fake_scene_bytes
correctness_scene_sha256 = "$fake_scene_sha256"
correctness_width = 1
correctness_height = 1
correctness_rays_per_pixel = 1
correctness_threads = 1
correctness_default_warmups = 0
correctness_default_samples = 1
performance_scene = "sphfract"
performance_scene_bytes = $fake_scene_bytes
performance_scene_sha256 = "$fake_scene_sha256"
performance_width = 1
performance_height = 1
performance_rays_per_pixel = 1
performance_threads = 1
performance_default_warmups = 0
performance_default_samples = 1
EOF

fake_platform_arguments=()
case "$(uname -s):$(uname -m)" in
  Darwin:arm64 | Darwin:aarch64)
    fake_platform=darwin-arm64
    fake_expected_target=aarch64-apple-darwin
    fake_platform_arguments=(--sdk-root "$fake_sdk")
    ;;
  Linux:x86_64)
    fake_platform=linux-x86_64
    fake_expected_target=x86_64-unknown-linux-gnu
    ;;
  *)
    echo "unsupported host for the C-Ray fake-run regression" >&2
    exit 1
    ;;
esac

fake_events="$fake_run_root/events.txt"
fake_ccc_arguments="$fake_run_root/ccc-compile-arguments.txt"
fake_locales="$fake_run_root/compiler-locales.txt"
fake_tool_events="$fake_run_root/tool-events.txt"
: >"$fake_events"
: >"$fake_ccc_arguments"
: >"$fake_locales"
: >"$fake_tool_events"
fake_ccc="$fake_tools/ccc"
cat >"$fake_ccc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for argument in "$@"; do
  case "$argument" in
    --version)
      printf 'CCC fake phase runner\n'
      exit 0
      ;;
    -dM)
      cat <<'MACROS'
#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__
#define __SIZEOF_POINTER__ 8
MACROS
      case "$FAKE_PLATFORM" in
        darwin-arm64)
          cat <<'MACROS'
#define __aarch64__ 1
#define __APPLE__ 1
MACROS
          ;;
        linux-x86_64)
          printf '#define __x86_64__ 1\n'
          ;;
        *)
          echo "unknown fake platform: $FAKE_PLATFORM" >&2
          exit 2
          ;;
      esac
      exit 0
      ;;
    --emit=codegen-stats)
      cat <<'STATS'
schema_version	3
post_inline_ir.functions	1
post_inline_ir.blocks	1
post_inline_ir.values	1
post_inline_ir.instructions	1
post_inline_ir.call_instructions	0
post_inline_ir.fixed_stack_slots	0
post_inline_ir.fixed_stack_bytes	0
post_inline_ir.dynamic_stack_slots	0
post_inline_ir.signatures	1
post_inline_ir.unused_signatures	0
post_inline_ir.external_functions	0
post_inline_ir.unused_external_functions	0
post_inline_ir.global_values	0
post_inline_ir.unused_global_values	0
post_inline_ir.constants	0
post_inline_ir.jump_tables	0
primary_object.file_bytes	16
primary_object.sections	1
primary_object.symbols	1
primary_object.defined_symbols	1
primary_object.undefined_symbols	0
primary_object.relocations	0
primary_object.text_bytes	16
primary_object.read_only_data_bytes	0
primary_object.writable_data_bytes	0
primary_object.bss_bytes	0
primary_object.tls_data_bytes	0
primary_object.tls_bss_bytes	0
primary_object.unwind_bytes	0
primary_object.debug_bytes	0
primary_object.metadata_bytes	0
primary_object.other_section_bytes	0
STATS
      exit 0
      ;;
  esac
done

output=
sidecar=
optimization=
compile=false
arguments=("$@")
for ((index = 0; index < ${#arguments[@]}; index++)); do
  argument=${arguments[$index]}
  case "$argument" in
    -c) compile=true ;;
    -O0 | -O2 | -Oz) optimization=$argument ;;
    -o)
      ((index++))
      output=${arguments[$index]}
      ;;
    --write-phase-timings=*)
      sidecar=${argument#*=}
      ;;
  esac
done
[[ "$compile" == true && -n "$output" && -n "$optimization" ]]
{
  printf 'BEGIN\n'
  printf '%s\n' "$@"
  printf 'END\n'
} >>"$FAKE_CCC_ARGUMENTS"
if [[ -n "$sidecar" ]]; then
  printf 'ccc-instrumented:%s\n' "$optimization" >>"$FAKE_EVENTS"
  compile_kind=instrumented
else
  printf 'ccc-measured:%s\n' "$optimization" >>"$FAKE_EVENTS"
  compile_kind=measured
fi
printf 'ccc-%s:%s:%s\n' \
  "$compile_kind" "$optimization" "${LC_ALL-unset}" >>"$FAKE_LOCALES"
mkdir -p "$(dirname -- "$output")"
printf 'stable-%s-object\n' "$optimization" >"$output"
if [[ -n "$sidecar" ]]; then
  mkdir -p "$(dirname -- "$sidecar")"
  cat >"$sidecar" <<'TIMINGS'
schema_version	1
preprocessing	11
parsing	13
semantic_analysis	17
ccc_ir_lowering	19
ccc_ir_optimization	23
codegen.total	29
object_packaging	31
pipeline	37
TIMINGS
fi
EOF
chmod +x "$fake_ccc"

fake_reference="$fake_tools/clang"
cat >"$fake_reference" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for argument in "$@"; do
  case "$argument" in
    --version)
      case "$FAKE_PLATFORM" in
        darwin-arm64) printf 'Apple Clang fake runner\n' ;;
        linux-x86_64) printf 'GCC fake runner\n' ;;
      esac
      exit 0
      ;;
    -dumpmachine)
      case "$FAKE_PLATFORM" in
        darwin-arm64) printf 'arm64-apple-darwin\n' ;;
        linux-x86_64) printf 'x86_64-linux-gnu\n' ;;
      esac
      exit 0
      ;;
    -dM)
      cat <<'MACROS'
#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__
#define __SIZEOF_POINTER__ 8
MACROS
      case "$FAKE_PLATFORM" in
        darwin-arm64)
          cat <<'MACROS'
#define __aarch64__ 1
#define __APPLE__ 1
MACROS
          ;;
        linux-x86_64)
          printf '#define __x86_64__ 1\n'
          ;;
      esac
      exit 0
      ;;
  esac
done

output=
compile=false
input_object=
arguments=("$@")
for ((index = 0; index < ${#arguments[@]}; index++)); do
  argument=${arguments[$index]}
  case "$argument" in
    -c) compile=true ;;
    *.o) input_object=$argument ;;
    -o)
      ((index++))
      output=${arguments[$index]}
      ;;
  esac
done
[[ -n "$output" ]]
mkdir -p "$(dirname -- "$output")"
if [[ "$compile" == true ]]; then
  printf 'reference-compile\n' >>"$FAKE_EVENTS"
  printf 'reference-compile:%s\n' "${LC_ALL-unset}" >>"$FAKE_LOCALES"
  printf 'reference object\n' >"$output"
  exit 0
fi
printf 'reference-link:%s\n' "$(basename -- "$input_object")" >>"$FAKE_EVENTS"
printf 'reference-link:%s:%s\n' \
  "$(basename -- "$input_object")" "${LC_ALL-unset}" >>"$FAKE_LOCALES"
cat >"$output" <<'PROGRAM'
#!/usr/bin/env bash
set -euo pipefail
destination=
while (($#)); do
  if [[ "$1" == -o ]]; then
    destination=$2
    shift 2
  else
    shift
  fi
done
[[ -n "$destination" ]]
printf 'P6\n1 1\n255\nabc' >"$destination"
PROGRAM
chmod +x "$output"
EOF
chmod +x "$fake_reference"

fake_size_tool="$fake_tools/llvm-size"
cat >"$fake_size_tool" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == --version ]]; then
  printf 'size-version:%s\n' "$FAKE_PLATFORM" >>"$FAKE_TOOL_EVENTS"
  printf 'llvm-size fake runner\n'
  exit 0
fi
if [[ "$1" == --format=sysv ]]; then
  artifact=$2
  printf '%s  :\n' "$artifact"
  cat <<'OUTPUT'
section    size   addr
__text       16      0
Total        16
OUTPUT
  exit 0
fi
printf 'text data bss dec hex filename\n'
printf '16 0 0 16 10 %s\n' "$1"
EOF
chmod +x "$fake_size_tool"
cp "$fake_size_tool" "$fake_tools/size"

fake_xcrun="$fake_tools/xcrun"
cat >"$fake_xcrun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == --find && "$2" == llvm-size ]]; then
  printf 'xcrun-find:%s\n' "$FAKE_PLATFORM" >>"$FAKE_TOOL_EVENTS"
  printf '%s\n' "$FAKE_SIZE_TOOL"
  exit 0
fi
echo "unexpected fake xcrun arguments: $*" >&2
exit 2
EOF
chmod +x "$fake_xcrun"

fake_work="$fake_run_root/results"
PATH="$fake_tools:$PATH" \
FAKE_CCC_ARGUMENTS="$fake_ccc_arguments" \
FAKE_EVENTS="$fake_events" \
FAKE_LOCALES="$fake_locales" \
FAKE_PLATFORM="$fake_platform" \
FAKE_SIZE_TOOL="$fake_size_tool" \
FAKE_TOOL_EVENTS="$fake_tool_events" \
  "$fake_adapter/run.sh" \
  --profile correctness \
  --source-archive "$fake_archive" \
  --work-dir "$fake_work" \
  --warmups 0 \
  --samples 1 \
  --ccc "$fake_ccc" \
  --resource-dir "$fake_resource_directory" \
  --reference-cc "$fake_reference" \
  "${fake_platform_arguments[@]}" \
  >/dev/null

grep -Fxq "target=$fake_expected_target" "$fake_work/run-config.txt"
case "$fake_platform" in
  darwin-arm64)
    grep -Fxq "sdk_root=$fake_sdk" "$fake_work/run-config.txt"
    grep -Fxq 'xcrun-find:darwin-arm64' "$fake_tool_events"
    ;;
  linux-x86_64)
    if grep -Fq 'sdk_root=' "$fake_work/run-config.txt"; then
      echo "Linux fake run unexpectedly recorded a macOS SDK" >&2
      exit 1
    fi
    if grep -q '^xcrun-find:' "$fake_tool_events"; then
      echo "Linux fake run unexpectedly used xcrun" >&2
      exit 1
    fi
    ;;
esac
grep -Fxq "size-version:$fake_platform" "$fake_tool_events"
[[ "$(grep -c ':C$' "$fake_locales")" == 11 ]]
if grep -v ':C$' "$fake_locales" >/dev/null; then
  echo "a measured or instrumented compiler command did not observe LC_ALL=C" >&2
  exit 1
fi
[[ "$(grep -c $'^compile\t' "$fake_work/timings.tsv")" == 4 ]]
[[ "$(grep -c $'^link\t' "$fake_work/timings.tsv")" == 4 ]]
[[ "$(grep -c $'^ccc-' "$fake_work/compile-phase-artifacts.tsv")" == 3 ]]
[[ "$(wc -l <"$fake_work/compile-phase-timings.tsv" | tr -d '[:space:]')" == 28 ]]
if grep -Eq 'phase|instrument' "$fake_work/timings.tsv"; then
  echo "untimed phase instrumentation leaked into timings.tsv" >&2
  exit 1
fi
cat >"$fake_run_root/expected-events.txt" <<'EOF'
ccc-measured:-O0
reference-link:ccc-o0.o
ccc-instrumented:-O0
ccc-measured:-O2
reference-link:ccc-o2.o
ccc-instrumented:-O2
ccc-measured:-Oz
reference-link:ccc-oz.o
ccc-instrumented:-Oz
reference-compile
reference-link:reference-o2.o
EOF
cmp "$fake_events" "$fake_run_root/expected-events.txt"
python3 - "$fake_ccc_arguments" "$fake_work/summary.tsv" <<'PY'
import csv
import sys

arguments = []
current = None
with open(sys.argv[1], encoding="utf-8") as source:
    for line in source:
        line = line.rstrip("\n")
        if line == "BEGIN":
            assert current is None
            current = []
        elif line == "END":
            assert current is not None
            arguments.append(current)
            current = None
        else:
            assert current is not None
            current.append(line)
assert current is None
assert len(arguments) == 6

def normalized(command):
    result = []
    index = 0
    while index < len(command):
        argument = command[index]
        if argument.startswith("--write-phase-timings="):
            index += 1
            continue
        result.append(argument)
        if argument == "-o":
            index += 1
            result.append("<object>")
        index += 1
    return result

for optimization in ("-O0", "-O2", "-Oz"):
    matching = [command for command in arguments if optimization in command]
    assert len(matching) == 2
    measured = next(
        command
        for command in matching
        if not any(
            argument.startswith("--write-phase-timings=")
            for argument in command
        )
    )
    instrumented = next(command for command in matching if command is not measured)
    assert "-c" in measured and "-c" in instrumented
    assert normalized(measured) == normalized(instrumented)

with open(sys.argv[2], encoding="utf-8", newline="") as source:
    rows = {row["label"]: row for row in csv.DictReader(source, delimiter="\t")}
phase_fields = [
    field for field in rows["ccc-o0"] if field.startswith("compile_phase_")
]
assert len(phase_fields) == 8
for label in ("ccc-o0", "ccc-o2", "ccc-oz"):
    assert all(rows[label][field] for field in phase_fields)
assert all(rows["reference-o2"][field] == "" for field in phase_fields)
PY
for label in ccc-o0 ccc-o2 ccc-oz; do
  raw="$fake_work/phase-timing-raw/$label"
  [[ -s "$raw/phase-timings.o" ]]
  [[ -f "$raw/phase-timings.tsv" ]]
  [[ -f "$raw/phase-timings.stdout.txt" ]]
  [[ -f "$raw/phase-timings.stderr.txt" ]]
  [[ -f "$raw/phase-timings.command.txt" ]]
  grep -Fq '"timed":false' "$raw/phase-timings.result.json"
done

bash -n "$script_directory/run.sh"

echo "C-Ray runner and result-tool regressions passed"
