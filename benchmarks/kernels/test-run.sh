#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-kernel-benchmark-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) native_target=x86_64-unknown-linux-gnu ;;
  Linux:aarch64 | Linux:arm64) native_target=aarch64-unknown-linux-gnu ;;
  Linux:riscv64) native_target=riscv64-unknown-linux-gnu ;;
  Darwin:arm64 | Darwin:aarch64) native_target=aarch64-apple-darwin ;;
  *)
    echo "kernel runner self-test needs an enabled Unix host profile" >&2
    exit 1
    ;;
esac

kernels=(
  direct-call
  integer-loop
  floating-loop
  branch-switch
  memory-traffic
  aggregate-copy
  tls-access
  atomic-rmw
  variadic-call
  crc32
  matrix-multiply
  heap-sort
  dijkstra
  image-stencil
)
kernel_count=${#kernels[@]}
performance_sample_count=$((kernel_count * 3 * 2))

fake_ccc="$temporary_directory/fake-ccc"
cat >"$fake_ccc" <<'PYTHON'
#!/usr/bin/env python3

import hashlib
import os
from pathlib import Path
import re
import sys


target = os.environ.get("FAKE_TARGET", "x86_64-unknown-linux-gnu")

if sys.argv[1:] == ["--version"]:
    print("fake-ccc 1.0")
    sys.exit(0)

if "-dumpmachine" in sys.argv:
    requested = [
        argument.split("=", 1)[1]
        for argument in sys.argv[1:]
        if argument.startswith("--target=")
    ]
    print(requested[0] if requested else target)
    sys.exit(0)

if "--print-effective-config" in sys.argv:
    selected = next(
        argument.split("=", 1)[1]
        for argument in sys.argv[1:]
        if argument.startswith("--target=")
    )
    profile = next(argument for argument in sys.argv if argument.startswith("-O"))
    print(f"target={selected}")
    print(f"optimization={profile}")
    print("compiler-driver=/usr/bin/fake-cc")
    sys.exit(0)

source_arguments = [argument for argument in sys.argv[1:] if argument.endswith(".c")]
profile = next(
    (argument for argument in sys.argv if argument.startswith("-O")),
    "-O0",
)
case_name = None
if len(source_arguments) == 1:
    source_text = Path(source_arguments[0]).read_text(encoding="utf-8")
    case_match = re.search(r"ccc-kernel-benchmark: ([a-z0-9-]+)", source_text)
    if case_match:
        case_name = case_match.group(1)

if "--emit=codegen-stats" in sys.argv:
    if len(source_arguments) != 1:
        print("expected one kernel source for stats", file=sys.stderr)
        sys.exit(2)
    structures = {
        "direct-call": {
            "functions": 2,
            "calls": 0 if profile == "-O2" else 1,
            "blocks": 5,
            "instructions": 24,
            "global_values": 1,
            "constants": 4,
            "jump_tables": 0,
        },
        "integer-loop": {
            "functions": 1,
            "calls": 0,
            "blocks": 4,
            "instructions": 20,
            "global_values": 1,
            "constants": 3,
            "jump_tables": 0,
        },
        "floating-loop": {
            "functions": 1,
            "calls": 0,
            "blocks": 4,
            "instructions": 18,
            "global_values": 4,
            "constants": 4,
            "jump_tables": 0,
        },
        "branch-switch": {
            "functions": 1,
            "calls": 0,
            "blocks": 15,
            "instructions": 45,
            "global_values": 1,
            "constants": 4,
            "jump_tables": 0,
        },
        "memory-traffic": {
            "functions": 1,
            "calls": 0,
            "blocks": 7,
            "instructions": 38,
            "global_values": 3,
            "constants": 5,
            "jump_tables": 0,
        },
        "aggregate-copy": {
            "functions": 1,
            "calls": 0,
            "blocks": 10,
            "instructions": 92,
            "global_values": 4,
            "constants": 7,
            "jump_tables": 0,
        },
        "tls-access": {
            "functions": 1,
            "calls": 4 if profile == "-O0" else 2,
            "blocks": 4,
            "instructions": 28,
            "global_values": 2,
            "constants": 5,
            "jump_tables": 0,
        },
        "atomic-rmw": {
            "functions": 1,
            "calls": 0,
            "blocks": 6,
            "instructions": 39,
            "global_values": 2,
            "constants": 6,
            "jump_tables": 0,
        },
        "variadic-call": {
            "functions": 2,
            "calls": 1,
            "blocks": 9,
            "instructions": 63,
            "global_values": 3,
            "constants": 8,
            "jump_tables": 0,
        },
        "crc32": {
            "functions": 1,
            "calls": 0,
            "blocks": 8,
            "instructions": 46,
            "global_values": 1,
            "constants": 5,
            "jump_tables": 0,
        },
        "matrix-multiply": {
            "functions": 1,
            "calls": 0,
            "blocks": 12,
            "instructions": 112,
            "global_values": 4,
            "constants": 9,
            "jump_tables": 0,
        },
        "heap-sort": {
            "functions": 1,
            "calls": 0,
            "blocks": 22,
            "instructions": 144,
            "global_values": 2,
            "constants": 10,
            "jump_tables": 0,
        },
        "dijkstra": {
            "functions": 1,
            "calls": 0,
            "blocks": 17,
            "instructions": 126,
            "global_values": 4,
            "constants": 8,
            "jump_tables": 0,
        },
        "image-stencil": {
            "functions": 1,
            "calls": 0,
            "blocks": 12,
            "instructions": 87,
            "global_values": 3,
            "constants": 7,
            "jump_tables": 0,
        },
    }
    if case_name not in structures:
        print(f"unknown fake kernel case: {case_name}", file=sys.stderr)
        sys.exit(2)
    structure = structures[case_name]
    metrics = [
        ("post_inline_ir.functions", structure["functions"]),
        ("post_inline_ir.blocks", structure["blocks"]),
        (
            "post_inline_ir.values",
            structure["instructions"] - structure["blocks"] + structure["functions"],
        ),
        ("post_inline_ir.instructions", structure["instructions"]),
        ("post_inline_ir.call_instructions", structure["calls"]),
        ("post_inline_ir.fixed_stack_slots", 0),
        ("post_inline_ir.fixed_stack_bytes", 0),
        ("post_inline_ir.dynamic_stack_slots", 0),
        ("post_inline_ir.signatures", structure["calls"]),
        ("post_inline_ir.unused_signatures", 0),
        ("post_inline_ir.external_functions", 0),
        ("post_inline_ir.unused_external_functions", 0),
        ("post_inline_ir.global_values", structure["global_values"]),
        ("post_inline_ir.unused_global_values", 0),
        ("post_inline_ir.constants", structure["constants"]),
        ("post_inline_ir.jump_tables", structure["jump_tables"]),
        ("primary_object.file_bytes", 41 + len(case_name)),
        ("primary_object.sections", 3),
        ("primary_object.symbols", structure["functions"] + 1),
        ("primary_object.defined_symbols", structure["functions"] + 1),
        ("primary_object.undefined_symbols", 0),
        ("primary_object.relocations", 1),
        ("primary_object.text_bytes", structure["instructions"]),
        ("primary_object.read_only_data_bytes", 0),
        ("primary_object.writable_data_bytes", 4),
        ("primary_object.bss_bytes", 0),
        ("primary_object.tls_data_bytes", 0),
        ("primary_object.tls_bss_bytes", 0),
        ("primary_object.unwind_bytes", 4),
        ("primary_object.debug_bytes", 0),
        ("primary_object.metadata_bytes", 4),
        ("primary_object.other_section_bytes", 0),
    ]
    print("schema_version\t3")
    for metric, value in metrics:
        if (
            os.environ.get("FAKE_CCC_DROP_UNUSED_METRIC")
            and metric == "post_inline_ir.unused_global_values"
        ):
            continue
        print(f"{metric}\t{value}")
    sys.exit(0)

if "-c" in sys.argv:
    if len(source_arguments) != 1 or "-o" not in sys.argv:
        print("malformed fake object compilation", file=sys.stderr)
        sys.exit(2)
    source = Path(source_arguments[0]).read_bytes()
    output = Path(sys.argv[sys.argv.index("-o") + 1])
    identity = hashlib.sha256(profile.encode() + b"\0" + source).digest()
    payload = b"fake-final-object\0" + identity
    if os.environ.get("FAKE_NONDETERMINISTIC_OBJECT"):
        payload += os.urandom(8)
    output.write_bytes(payload)
    sys.exit(0)

object_arguments = [argument for argument in sys.argv[1:] if argument.endswith(".o")]
if len(object_arguments) == 1 and "-o" in sys.argv:
    output = Path(sys.argv[sys.argv.index("-o") + 1])
    exit_status = int(os.environ.get("FAKE_PROGRAM_EXIT", "0"))
    output.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.exit({exit_status})\n",
        encoding="utf-8",
    )
    output.chmod(0o755)
    sys.exit(0)

print("unexpected fake compiler invocation", file=sys.stderr)
sys.exit(2)
PYTHON
chmod +x "$fake_ccc"

fake_runner="$temporary_directory/fake-runner"
cat >"$fake_runner" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
exec "$@"
SH
chmod +x "$fake_runner"

performance_results="$temporary_directory/performance"
FAKE_TARGET="$native_target" "$script_directory/run.py" \
  --ccc "$fake_ccc" \
  --output "$performance_results" \
  --mode performance \
  --profiles O0,O2,Oz \
  --compile-warmups 1 \
  --compile-samples 2 \
  --run-warmups 1 \
  --run-samples 2

grep -Fq $'benchmark\tfamily\tprofile\ttarget\tmode\texecution_kind' \
  "$performance_results/summary.tsv"
grep -Fq $'direct-call\tdirect-calls\tO2\t'"$native_target"$'\tperformance\tnative\t1' \
  "$performance_results/summary.tsv"
for kernel in "${kernels[@]}"; do
  expected_functions=1
  case "$kernel" in
    direct-call)
      family=direct-calls
      expected_functions=2
      ;;
    integer-loop) family=integer-loops ;;
    floating-loop) family=floating-loops ;;
    branch-switch) family=branches-switches ;;
    memory-traffic) family=memory-loads-stores ;;
    aggregate-copy) family=aggregate-copies ;;
    tls-access) family=thread-local-storage ;;
    atomic-rmw) family=c11-atomics ;;
    variadic-call)
      family=variadic-abi
      expected_functions=2
      ;;
    crc32) family=checksums ;;
    matrix-multiply) family=dense-linear-algebra ;;
    heap-sort) family=comparison-sorting ;;
    dijkstra) family=shortest-path-routing ;;
    image-stencil) family=image-stencils ;;
  esac
  for profile in O0 O2 Oz; do
    expected_calls=0
    if [[ "$kernel" == direct-call && "$profile" != O2 ]]; then
      expected_calls=1
    fi
    if [[ "$kernel" == variadic-call ]]; then
      expected_calls=1
    fi
    if [[ "$kernel" == tls-access ]]; then
      expected_calls=2
      if [[ "$profile" == O0 ]]; then
        expected_calls=4
      fi
    fi
    grep -Fq \
      "$kernel"$'\t'"$family"$'\t'"$profile"$'\tpost_inline_ir.functions\t'"$expected_functions" \
      "$performance_results/codegen-stats.tsv"
    grep -Fq \
      "$kernel"$'\t'"$family"$'\t'"$profile"$'\tpost_inline_ir.call_instructions\t'"$expected_calls" \
      "$performance_results/codegen-stats.tsv"
  done
done
grep -Fq $'direct-call\tO2\tfinal-object\t' \
  "$performance_results/artifacts.tsv"
grep -Fq $'direct-call\tO2\texecutable\t' \
  "$performance_results/artifacts.tsv"
grep -Fq '"format_version":3' "$performance_results/environment.json"
grep -Fq '"mode":"performance"' "$performance_results/environment.json"
grep -Fq '"kind":"link"' "$performance_results/commands.jsonl"
grep -Fq '"phase":"validation"' "$performance_results/commands.jsonl"
[[ "$(grep -c $'\tsample\t' "$performance_results/run-times.tsv")" == \
  "$performance_sample_count" ]]
[[ "$(find "$performance_results/raw" -name 'compile-sample-*.o' |
  wc -l | tr -d '[:space:]')" == "$performance_sample_count" ]]

object_results="$temporary_directory/object"
FAKE_TARGET="$native_target" "$script_directory/run.py" \
  --ccc "$fake_ccc" \
  --output "$object_results" \
  --mode object \
  --profiles O2 \
  --compile-samples 1
[[ "$(wc -l <"$object_results/run-times.tsv" | tr -d '[:space:]')" == 1 ]]
[[ "$(wc -l <"$object_results/summary.tsv" | tr -d '[:space:]')" == \
  "$((kernel_count + 1))" ]]
if grep -Fq $'\texecutable\t' "$object_results/artifacts.tsv"; then
  echo "object mode unexpectedly produced an executable artifact" >&2
  exit 1
fi
grep -Fq $'\tobject\tnot-run\t0\t' "$object_results/summary.tsv"

case "$native_target" in
  riscv64-unknown-linux-gnu) cross_target=x86_64-unknown-linux-gnu ;;
  *) cross_target=riscv64-unknown-linux-gnu ;;
esac
correctness_results="$temporary_directory/correctness"
FAKE_TARGET="$cross_target" "$script_directory/run.py" \
  --ccc "$fake_ccc" \
  --output "$correctness_results" \
  --mode correctness \
  --target "$cross_target" \
  --profiles O0 \
  --runner "$fake_runner"
grep -Fq $'\tcorrectness\trunner\t0\t' "$correctness_results/summary.tsv"
grep -Fq $'\trunner\tvalidation\t1\t' "$correctness_results/run-times.tsv"
[[ "$(grep -c $'\trunner\tvalidation\t' \
  "$correctness_results/run-times.tsv")" == "$kernel_count" ]]

nonzero_results="$temporary_directory/nonzero"
set +e
nonzero_output=$(
  FAKE_TARGET="$native_target" FAKE_PROGRAM_EXIT=7 \
    "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$nonzero_results" \
    --mode correctness \
    --profiles O0 2>&1
)
nonzero_status=$?
set -e
[[ "$nonzero_status" == 1 ]]
[[ "$nonzero_output" == *"validation failed with status 7"* ]]
grep -Fq $'\tvalidation\t1\t' "$nonzero_results/run-times.tsv"
grep -Fq $'\t7' "$nonzero_results/run-times.tsv"

nondeterministic_results="$temporary_directory/nondeterministic"
set +e
nondeterministic_output=$(
  FAKE_TARGET="$native_target" FAKE_NONDETERMINISTIC_OBJECT=1 \
    "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$nondeterministic_results" \
    --mode object \
    --profiles O0 \
    --compile-samples 2 2>&1
)
nondeterministic_status=$?
set -e
[[ "$nondeterministic_status" == 1 ]]
[[ "$nondeterministic_output" == *"produced nondeterministic final objects"* ]]

missing_stats_results="$temporary_directory/missing-stats"
set +e
missing_stats_output=$(
  FAKE_TARGET="$native_target" FAKE_CCC_DROP_UNUSED_METRIC=1 \
    "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$missing_stats_results" \
    --mode object \
    --profiles O0 \
    --compile-samples 1 2>&1
)
missing_stats_status=$?
set -e
[[ "$missing_stats_status" == 1 ]]
[[ "$missing_stats_output" == *"invalid codegen-stats schema: missing post_inline_ir.unused_global_values"* ]]

help_output=$("$script_directory/run.py" --help)
[[ "$help_output" == *"--mode"* ]]
[[ "$help_output" == *"--runner-arg"* ]]
[[ "$help_output" == *"--compile-samples"* ]]
