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

if "--emit=codegen-stats" not in sys.argv:
    if "-c" not in sys.argv or "-o" not in sys.argv:
        print("expected an object-only compilation", file=sys.stderr)
        sys.exit(2)
    output = Path(sys.argv[sys.argv.index("-o") + 1])
    profile = next(argument for argument in sys.argv if argument.startswith("-O"))
    identity = hashlib.sha256(f"{profile}\n{source}".encode()).digest()
    payload = b"fake-object\0" + identity
    if (
        family == "data-declaration-heavy"
        and os.environ.get("FAKE_CCC_LEAK_DATA_DECLS")
    ):
        payload += b"x" * scale
    output.write_bytes(payload)
    sys.exit(0)

functions = scale + 1 if family == "live-functions" else 1
if family == "declaration-heavy" and os.environ.get("FAKE_CCC_LEAK_DECLS"):
    functions += scale
calls = 1 if family in ("puts-call", "printf-variadic") else 0
if family == "live-functions":
    calls = scale
object_file_bytes = 44
object_undefined_symbols = calls
if (
    family == "data-declaration-heavy"
    and os.environ.get("FAKE_CCC_LEAK_DATA_DECLS")
):
    object_file_bytes += scale
    object_undefined_symbols += scale
object_symbols = functions + object_undefined_symbols

metrics = [
    ("post_inline_ir.functions", functions),
    ("post_inline_ir.blocks", functions),
    ("post_inline_ir.instructions", functions * 3 + calls),
    ("post_inline_ir.call_instructions", calls),
    ("post_inline_ir.fixed_stack_slots", 0),
    ("post_inline_ir.fixed_stack_bytes", 0),
    ("post_inline_ir.dynamic_stack_slots", 0),
    ("post_inline_ir.signatures", calls),
    ("post_inline_ir.external_functions", calls),
    ("post_inline_ir.global_values", 0),
    ("post_inline_ir.constants", 0),
    ("post_inline_ir.jump_tables", 0),
    ("primary_object.file_bytes", object_file_bytes),
    ("primary_object.sections", 3),
    ("primary_object.symbols", object_symbols),
    ("primary_object.defined_symbols", functions),
    ("primary_object.undefined_symbols", object_undefined_symbols),
    ("primary_object.relocations", calls),
    ("primary_object.text_bytes", functions * 8),
    ("primary_object.read_only_data_bytes", 0),
    ("primary_object.writable_data_bytes", 0),
    ("primary_object.bss_bytes", 0),
    ("primary_object.tls_data_bytes", 0),
    ("primary_object.tls_bss_bytes", 0),
    ("primary_object.unwind_bytes", functions * 2),
    ("primary_object.debug_bytes", 0),
    ("primary_object.metadata_bytes", 4),
    ("primary_object.other_section_bytes", 0),
]
print("schema_version\t1")
for metric, value in metrics:
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
  --function-scales 2,4

grep -Fq $'benchmark\tfamily\tscale\tprofile\tphase\titeration\twall_seconds' \
  "$results/compile-times.tsv"
grep -Fq $'declaration-heavy-3\tdeclaration-heavy\t3\tO2\tpost_inline_ir.functions\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'data-declaration-heavy-3\tdata-declaration-heavy\t3\tO2\tprimary_object.symbols\t1' \
  "$results/codegen-stats.tsv"
grep -Fq $'live-functions-4\tlive-functions\t4\tO2\t2\t' \
  "$results/summary.tsv"
grep -Fq '"format_version":1' "$results/environment.json"
expected_compiler_sha=$(
  python3 -c \
    'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' \
    "$fake_ccc"
)
grep -Fq "\"compiler_sha256\":\"$expected_compiler_sha\"" \
  "$results/environment.json"
grep -Fq '"target":"x86_64-unknown-linux-gnu"' "$results/environment.json"
grep -Fq '"data_declaration_scales":[0,3]' "$results/environment.json"
grep -Fq '"effective_configs":{"O0":' "$results/environment.json"
grep -Fq '"exit_status":0' "$results/environment.json"
grep -Fq '"benchmark":"printf-variadic"' "$results/commands.jsonl"
[[ "$(grep -c '"kind":"object-compile"' "$results/commands.jsonl")" == 54 ]]
[[ "$(grep -c '"kind":"codegen-stats"' "$results/commands.jsonl")" == 18 ]]
[[ "$(grep -c '^extern long ccc_decl_' \
  "$results/sources/declaration-heavy-3.c")" == 3 ]]
[[ "$(grep -c '^extern long ccc_data_decl_' \
  "$results/sources/data-declaration-heavy-3.c")" == 3 ]]
[[ "$(find "$results/raw" -name 'codegen-stats.stdout.tsv' |
  wc -l | tr -d '[:space:]')" == 18 ]]
[[ "$(find "$results/raw" -name '*.o' | wc -l | tr -d '[:space:]')" == 54 ]]
[[ -f "$results/raw/printf-variadic/O2/sample-002.timing.json" ]]
[[ -s "$results/raw/printf-variadic/O2/sample-002.o" ]]
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

help_output=$("$script_directory/run.py" --help)
[[ "$help_output" == *"--declaration-scales"* ]]
[[ "$help_output" == *"--data-declaration-scales"* ]]
[[ "$help_output" == *"--function-scales"* ]]

PYTHONPYCACHEPREFIX="$temporary_directory/pycache" \
  python3 -m py_compile "$script_directory/run.py"
bash -n "$script_directory/test-run.sh"

echo "codegen benchmark regression passed"
