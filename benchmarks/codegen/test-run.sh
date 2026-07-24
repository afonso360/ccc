#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-codegen-benchmark-test.XXXXXX")
cleanup() {
  rm -rf -- "$temporary_directory"
}
trap cleanup EXIT

fake_ccc="$temporary_directory/fake-ccc"
cat >"$fake_ccc" <<'PYTHON'
#!/usr/bin/env python3

import hashlib
import os
from pathlib import Path
import re
import sys

if sys.argv[1:] == ["--version"]:
    print("fake-ccc 1.0")
    sys.exit(0)

if "-dumpmachine" in sys.argv:
    targets = [
        argument.split("=", 1)[1]
        for argument in sys.argv[1:]
        if argument.startswith("--target=")
    ]
    print(targets[0] if targets else "x86_64-unknown-linux-gnu")
    sys.exit(0)

if "--print-effective-config" in sys.argv:
    target = next(
        argument.split("=", 1)[1]
        for argument in sys.argv[1:]
        if argument.startswith("--target=")
    )
    profile = next(argument for argument in sys.argv if argument.startswith("-O"))
    print(f"target={target}")
    print(f"optimization={profile}")
    print("compiler-driver=/usr/bin/fake-cc")
    print("sysroot=/fake/sysroot")
    sys.exit(0)

source_arguments = [argument for argument in sys.argv[1:] if argument.endswith(".c")]
if len(source_arguments) != 1:
    print("unexpected fake compiler invocation", file=sys.stderr)
    sys.exit(2)

source = Path(source_arguments[0]).read_text(encoding="utf-8")
family = re.search(r"ccc-benchmark-family: ([a-z-]+)", source).group(1)
scale = int(re.search(r"ccc-benchmark-scale: ([0-9]+)", source).group(1))
variant_match = re.search(r"ccc-benchmark-variant: ([a-z-]+)", source)
variant = variant_match.group(1) if variant_match else None
hosted_family = family in ("hosted-header", "hosted-printf")
hosted_stdio = hosted_family and variant == "stdio"

if "--emit=codegen-stats" not in sys.argv:
    if "-c" not in sys.argv or "-o" not in sys.argv:
        print("expected an object-only compilation", file=sys.stderr)
        sys.exit(2)
    output = Path(sys.argv[sys.argv.index("-o") + 1])
    profile = next(argument for argument in sys.argv if argument.startswith("-O"))
    identity = hashlib.sha256(f"{profile}\n{source}".encode()).digest()
    payload = b"fake-object\0" + identity
    if family == "hosted-printf":
        # Model CCC's ABI-support packaging: codegen stats describe the primary
        # Cranelift object, while an ordinary -c output can include bridge code.
        payload += b"packaged-variadic-bridge"
    if (
        family == "data-declaration-heavy"
        and os.environ.get("FAKE_CCC_LEAK_DATA_DECLS")
    ):
        payload += b"x" * scale
    output.write_bytes(payload)
    sys.exit(0)

scaling_families = {
    "block-count",
    "ssa-values",
    "live-globals",
    "string-literals",
}
functions = scale + 1 if family == "live-functions" else 1
if family in scaling_families:
    functions = 2
if family == "declarations-per-function":
    functions = 5
if family == "declaration-heavy" and os.environ.get("FAKE_CCC_LEAK_DECLS"):
    functions += scale
calls = 1 if family in ("puts-call", "printf-variadic") else 0
if family == "live-functions":
    calls = scale
if family in scaling_families:
    calls = 1
if family == "declarations-per-function":
    calls = 4
if hosted_family:
    calls = 1
external_functions = calls
if hosted_stdio and os.environ.get("FAKE_CCC_HOSTED_IR_LEAK"):
    external_functions += 1
object_file_bytes = 44
object_undefined_symbols = calls
object_relocations = calls
if family == "hosted-header":
    object_undefined_symbols = 2
if (
    family == "data-declaration-heavy"
    and os.environ.get("FAKE_CCC_LEAK_DATA_DECLS")
):
    object_file_bytes += scale
    object_undefined_symbols += scale
if hosted_stdio and os.environ.get("FAKE_CCC_HOSTED_OBJECT_LEAK"):
    object_undefined_symbols += 1
    object_relocations += 1
if family == "declarations-per-function" and os.environ.get(
    "FAKE_CCC_LEAK_SCOPED_DECLS_OBJECT"
):
    object_undefined_symbols += scale * 4
    object_relocations += scale * 4
if family == "string-literals":
    object_relocations += scale
object_defined_symbols = functions
if family == "live-globals":
    object_defined_symbols += scale
object_symbols = object_defined_symbols + object_undefined_symbols
global_values = 2 if family == "hosted-header" else 0
if family in ("live-globals", "string-literals"):
    global_values = scale
blocks = functions
values = functions * 2 + calls
instructions = functions * 3 + calls
if family == "declarations-per-function" and os.environ.get(
    "FAKE_CCC_LEAK_SCOPED_DECLS_IR"
):
    values += scale * 4
if family == "block-count":
    blocks += scale * 2
    values += scale * 8
    instructions += scale * 10
if family == "ssa-values":
    values += scale * 2
    instructions += scale * 2
    if os.environ.get("FAKE_CCC_QUADRATIC_VALUES"):
        values += scale * scale
if family == "live-globals":
    values += scale * 4
    instructions += scale * 4
string_count = scale if family == "string-literals" else 0
if family == "string-literals" and os.environ.get(
    "FAKE_CCC_DROP_STRING_LITERAL"
):
    string_count = max(0, scale - 1)
    global_values = string_count
    object_relocations = calls + string_count

metrics = [
    ("post_inline_ir.functions", functions),
    ("post_inline_ir.blocks", blocks),
    ("post_inline_ir.values", values),
    ("post_inline_ir.instructions", instructions),
    ("post_inline_ir.call_instructions", calls),
    ("post_inline_ir.fixed_stack_slots", 0),
    ("post_inline_ir.fixed_stack_bytes", 0),
    ("post_inline_ir.dynamic_stack_slots", 0),
    ("post_inline_ir.signatures", calls),
    ("post_inline_ir.unused_signatures", 0),
    ("post_inline_ir.external_functions", external_functions),
    ("post_inline_ir.unused_external_functions", 0),
    ("post_inline_ir.global_values", global_values),
    ("post_inline_ir.unused_global_values", 0),
    ("post_inline_ir.constants", 0),
    ("post_inline_ir.jump_tables", 0),
    ("primary_object.file_bytes", object_file_bytes),
    ("primary_object.sections", 3),
    ("primary_object.symbols", object_symbols),
    ("primary_object.defined_symbols", object_defined_symbols),
    ("primary_object.undefined_symbols", object_undefined_symbols),
    ("primary_object.relocations", object_relocations),
    ("primary_object.text_bytes", functions * 8),
    ("primary_object.read_only_data_bytes", string_count * 18),
    (
        "primary_object.writable_data_bytes",
        scale * 4 if family == "live-globals" else 0,
    ),
    ("primary_object.bss_bytes", 0),
    ("primary_object.tls_data_bytes", 0),
    ("primary_object.tls_bss_bytes", 0),
    ("primary_object.unwind_bytes", functions * 2),
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
PYTHON
chmod +x "$fake_ccc"

results="$temporary_directory/results"
"$script_directory/run.py" \
  --ccc "$fake_ccc" \
  --output "$results" \
  --profiles O0,O2 \
  --warmups 1 \
  --samples 2 \
  --declaration-scales 0,3 \
  --data-declaration-scales 0,3 \
  --declarations-per-function-scales 0,2 \
  --function-scales 2,4 \
  --block-scales 0,2 \
  --value-scales 0,2 \
  --global-scales 0,2 \
  --string-scales 0,2

grep -Fq $'benchmark\tfamily\tscale\tprofile\tphase\titeration\twall_seconds' \
  "$results/compile-times.tsv"
grep -Fq $'declaration-heavy-3\tdeclaration-heavy\t3\tO2\tpost_inline_ir.functions\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'data-declaration-heavy-3\tdata-declaration-heavy\t3\tO2\tprimary_object.symbols\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'declarations-per-function-2\tdeclarations-per-function\t2\tO2\tpost_inline_ir.functions\t5' \
  "$results/codegen-stats.tsv"
grep -Fq $'hosted-header-stdio\thosted-header\t1\tO2\tpost_inline_ir.external_functions\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'hosted-printf-stdio\thosted-printf\t1\tO2\tpost_inline_ir.external_functions\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'live-functions-4\tlive-functions\t4\tO2\t2\t' \
  "$results/summary.tsv"
grep -Fq $'block-count-2\tblock-count\t2\tO2\tpost_inline_ir.blocks\t6' \
  "$results/codegen-stats.tsv"
grep -Fq $'ssa-values-2\tssa-values\t2\tO2\tpost_inline_ir.values\t9' \
  "$results/codegen-stats.tsv"
grep -Fq $'live-globals-2\tlive-globals\t2\tO2\tpost_inline_ir.global_values\t2' \
  "$results/codegen-stats.tsv"
grep -Fq $'string-literals-2\tstring-literals\t2\tO2\tprimary_object.read_only_data_bytes\t36' \
  "$results/codegen-stats.tsv"
grep -Fq '"format_version":5' "$results/environment.json"
grep -Fq '"codegen_stats_schema_version":3' "$results/environment.json"
expected_compiler_sha=$(
  python3 -c \
    'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' \
    "$fake_ccc"
)
grep -Fq "\"compiler_sha256\":\"$expected_compiler_sha\"" \
  "$results/environment.json"
grep -Fq '"target":"x86_64-unknown-linux-gnu"' "$results/environment.json"
grep -Fq '"data_declaration_scales":[0,3]' "$results/environment.json"
grep -Fq '"declarations_per_function_scales":[0,2]' \
  "$results/environment.json"
grep -Fq '"declarations_per_function_functions":4' \
  "$results/environment.json"
grep -Fq '"effective_configs":{"O0":' "$results/environment.json"
grep -Fq '"exit_status":0' "$results/environment.json"
grep -Fq '"benchmark":"printf-variadic"' "$results/commands.jsonl"
[[ "$(grep -c '"kind":"object-compile"' "$results/commands.jsonl")" == 138 ]]
[[ "$(grep -c '"kind":"codegen-stats"' "$results/commands.jsonl")" == 46 ]]
[[ "$(grep -c '^extern long ccc_decl_' \
  "$results/sources/declaration-heavy-3.c")" == 3 ]]
[[ "$(grep -c '^extern long ccc_data_decl_' \
  "$results/sources/data-declaration-heavy-3.c")" == 3 ]]
[[ "$(grep -c '^    extern unsigned ccc_scoped_decl_' \
  "$results/sources/declarations-per-function-2.c")" == 8 ]]
[[ "$(grep -c '^static unsigned ccc_declaration_scope_' \
  "$results/sources/declarations-per-function-2.c")" == 4 ]]
[[ "$(grep -c '^    if (selector ==' \
  "$results/sources/block-count-2.c")" == 2 ]]
[[ "$(grep -c '^    value = value \\*' \
  "$results/sources/ssa-values-2.c")" == 2 ]]
[[ "$(grep -c '^unsigned ccc_global_' \
  "$results/sources/live-globals-2.c")" == 2 ]]
[[ "$(grep -c '"ccc-string-' \
  "$results/sources/string-literals-2.c")" == 2 ]]
grep -Fq $'hosted-header-stdio\thosted-header\t1\thosted-header-minimal\t' \
  "$results/manifest.tsv"
grep -Fq $'hosted-printf-stdio\thosted-printf\t1\thosted-printf-minimal\t' \
  "$results/manifest.tsv"
grep -Fq '#include <stdio.h>' \
  "$results/sources/hosted-header-stdio.c"
grep -Fq 'extern struct ccc_benchmark_file *stdout;' \
  "$results/sources/hosted-header-minimal.c"
grep -Fq '#include <stdio.h>' \
  "$results/sources/hosted-printf-stdio.c"
grep -Fq 'extern int printf(const char *format, ...);' \
  "$results/sources/hosted-printf-minimal.c"
[[ "$(find "$results/raw" -name 'codegen-stats.stdout.tsv' |
  wc -l | tr -d '[:space:]')" == 46 ]]
[[ "$(find "$results/raw" -name '*.o' | wc -l | tr -d '[:space:]')" == 138 ]]
[[ -f "$results/raw/printf-variadic/O2/sample-002.timing.json" ]]
[[ -s "$results/raw/printf-variadic/O2/sample-002.o" ]]
[[ "$(wc -c <"$results/raw/hosted-printf-minimal/O2/sample-002.o" |
  tr -d '[:space:]')" -gt 44 ]]
[[ -f "$results/raw/printf-variadic/O2/sample-002.stdout.txt" ]]
[[ -f "$results/raw/minimal-return/O0/warmup-001.stderr.txt" ]]
[[ -f "$results/raw/minimal-return/O0/codegen-stats.result.json" ]]
[[ -f "$results/compiler-dumpmachine.stdout.txt" ]]
[[ -f "$results/effective-config/O0.stdout.txt" ]]
grep -Fq 'compiler-driver=/usr/bin/fake-cc' \
  "$results/effective-config/O2.stdout.txt"
[[ "$(cat "$results/compiler-dumpmachine.stdout.txt")" == \
  x86_64-unknown-linux-gnu ]]
timing_payload=$(cat "$results/raw/printf-variadic/O2/sample-002.timing.json")
[[ "$timing_payload" == *'"-c"'* ]]
[[ "$timing_payload" == *'"-o"'* ]]
[[ "$timing_payload" != *'--emit=codegen-stats'* ]]

negative_results="$temporary_directory/negative-results"
set +e
negative_output=$(
  FAKE_CCC_LEAK_DECLS=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_results" \
    --cases declaration-heavy \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --declaration-scales 2 2>&1
)
negative_status=$?
set -e
[[ "$negative_status" == 1 ]]
[[ "$negative_output" == *"emitted 3 post-inline functions; expected 1"* ]]

negative_data_results="$temporary_directory/negative-data-results"
set +e
negative_data_output=$(
  FAKE_CCC_LEAK_DATA_DECLS=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_data_results" \
    --cases data-declaration-heavy \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --data-declaration-scales 0,2 2>&1
)
negative_data_status=$?
set -e
[[ "$negative_data_status" == 1 ]]
[[ "$negative_data_output" == *"unused data declarations changed"* ]]
data_leak_prefix=$'data-declaration-heavy-2\tdata-declaration-heavy\t2\tO0\t'
grep -Fq "${data_leak_prefix}primary_object.symbols"$'\t3' \
  "$negative_data_results/codegen-stats.tsv"
grep -Fq "${data_leak_prefix}primary_object.undefined_symbols"$'\t2' \
  "$negative_data_results/codegen-stats.tsv"

negative_scoped_ir_results="$temporary_directory/negative-scoped-ir-results"
set +e
negative_scoped_ir_output=$(
  FAKE_CCC_LEAK_SCOPED_DECLS_IR=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_scoped_ir_results" \
    --cases declarations-per-function \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --declarations-per-function-scales 0,2 2>&1
)
negative_scoped_ir_status=$?
set -e
[[ "$negative_scoped_ir_status" == 1 ]]
[[ "$negative_scoped_ir_output" == *"unused declarations per function changed post-inline CLIF or primary-object structure at -O0"* ]]
[[ "$negative_scoped_ir_output" == *"post_inline_ir.values=14 versus 22"* ]]

negative_scoped_object_results="$temporary_directory/negative-scoped-object-results"
set +e
negative_scoped_object_output=$(
  FAKE_CCC_LEAK_SCOPED_DECLS_OBJECT=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_scoped_object_results" \
    --cases declarations-per-function \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --declarations-per-function-scales 0,2 2>&1
)
negative_scoped_object_status=$?
set -e
[[ "$negative_scoped_object_status" == 1 ]]
[[ "$negative_scoped_object_output" == *"unused declarations per function changed post-inline CLIF or primary-object structure at -O0"* ]]
[[ "$negative_scoped_object_output" == *"primary_object.relocations=4 versus 12"* ]]

negative_hosted_ir_results="$temporary_directory/negative-hosted-ir-results"
set +e
negative_hosted_ir_output=$(
  FAKE_CCC_HOSTED_IR_LEAK=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_hosted_ir_results" \
    --cases hosted-header \
    --profiles O0 \
    --warmups 0 \
    --samples 1 2>&1
)
negative_hosted_ir_status=$?
set -e
[[ "$negative_hosted_ir_status" == 1 ]]
[[ "$negative_hosted_ir_output" == *"hosted-header-stdio changed post-inline CLIF relative to hosted-header-minimal at -O0"* ]]
[[ "$negative_hosted_ir_output" == *"post_inline_ir.external_functions=1 versus 2"* ]]

negative_hosted_object_results="$temporary_directory/negative-hosted-object-results"
set +e
negative_hosted_object_output=$(
  FAKE_CCC_HOSTED_OBJECT_LEAK=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_hosted_object_results" \
    --cases hosted-header \
    --profiles O0 \
    --warmups 0 \
    --samples 1 2>&1
)
negative_hosted_object_status=$?
set -e
[[ "$negative_hosted_object_status" == 1 ]]
[[ "$negative_hosted_object_output" == *"hosted-header-stdio changed primary-object structure relative to hosted-header-minimal at -O0"* ]]
[[ "$negative_hosted_object_output" == *"primary_object.undefined_symbols=2 versus 3"* ]]

negative_printf_results="$temporary_directory/negative-printf-results"
set +e
negative_printf_output=$(
  FAKE_CCC_HOSTED_IR_LEAK=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_printf_results" \
    --cases hosted-printf \
    --profiles O0 \
    --warmups 0 \
    --samples 1 2>&1
)
negative_printf_status=$?
set -e
[[ "$negative_printf_status" == 1 ]]
[[ "$negative_printf_output" == *"hosted-printf-stdio changed post-inline CLIF relative to hosted-printf-minimal at -O0"* ]]
[[ "$negative_printf_output" == *"post_inline_ir.external_functions=1 versus 2"* ]]

negative_quadratic_results="$temporary_directory/negative-quadratic-results"
set +e
negative_quadratic_output=$(
  FAKE_CCC_QUADRATIC_VALUES=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_quadratic_results" \
    --cases ssa-values \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --value-scales 1,3 2>&1
)
negative_quadratic_status=$?
set -e
[[ "$negative_quadratic_status" == 1 ]]
[[ "$negative_quadratic_output" == *"ssa-values at -O0 produced non-linear structural growth for post_inline_ir.values"* ]]

negative_string_results="$temporary_directory/negative-string-results"
set +e
negative_string_output=$(
  FAKE_CCC_DROP_STRING_LITERAL=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$negative_string_results" \
    --cases string-literals \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --string-scales 0,2 2>&1
)
negative_string_status=$?
set -e
[[ "$negative_string_status" == 1 ]]
[[ "$negative_string_output" == *"string-literals at -O0 produced non-linear structural growth for post_inline_ir.global_values"* ]]

single_scale_results="$temporary_directory/single-scale-results"
set +e
single_scale_output=$(
  "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$single_scale_results" \
    --cases block-count \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --block-scales 2 2>&1
)
single_scale_status=$?
set -e
[[ "$single_scale_status" == 1 ]]
[[ "$single_scale_output" == *"block-count at -O0 requires at least two scales"* ]]

single_scoped_scale_results="$temporary_directory/single-scoped-scale-results"
set +e
single_scoped_scale_output=$(
  "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$single_scoped_scale_results" \
    --cases declarations-per-function \
    --profiles O0 \
    --warmups 0 \
    --samples 1 \
    --declarations-per-function-scales 2 2>&1
)
single_scoped_scale_status=$?
set -e
[[ "$single_scoped_scale_status" == 1 ]]
[[ "$single_scoped_scale_output" == *"declarations-per-function at -O0 requires at least two scales"* ]]

missing_stats_results="$temporary_directory/missing-stats-results"
set +e
missing_stats_output=$(
  FAKE_CCC_DROP_UNUSED_METRIC=1 "$script_directory/run.py" \
    --ccc "$fake_ccc" \
    --output "$missing_stats_results" \
    --cases minimal-return \
    --profiles O0 \
    --warmups 0 \
    --samples 1 2>&1
)
missing_stats_status=$?
set -e
[[ "$missing_stats_status" == 1 ]]
[[ "$missing_stats_output" == *"invalid codegen-stats schema: missing post_inline_ir.unused_global_values"* ]]

help_output=$("$script_directory/run.py" --help)
[[ "$help_output" == *"--declaration-scales"* ]]
[[ "$help_output" == *"--data-declaration-scales"* ]]
[[ "$help_output" == *"--declarations-per-function-scales"* ]]
[[ "$help_output" == *"--function-scales"* ]]
[[ "$help_output" == *"--block-scales"* ]]
[[ "$help_output" == *"--value-scales"* ]]
[[ "$help_output" == *"--global-scales"* ]]
[[ "$help_output" == *"--string-scales"* ]]

PYTHONPYCACHEPREFIX="$temporary_directory/pycache" \
  python3 -m py_compile "$script_directory/run.py"
bash -n "$script_directory/test-run.sh"

echo "codegen benchmark regression passed"
