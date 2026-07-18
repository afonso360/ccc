#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
while IFS='=' read -r variable _; do
  case "$variable" in
    CSMITH_* | CCC | CCC_* | FAKE_*) unset -v "$variable" ;;
  esac
done < <(env)
unset CDPATH
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-csmith-runner-test.XXXXXX")
cleanup() {
  local status=$?
  trap - EXIT
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    if ((status == 0)); then
      rm -rf -- "$temporary_directory"
    else
      printf 'Csmith harness artifacts retained at %s\n' \
        "$temporary_directory" >&2
    fi
  fi
  exit "$status"
}
trap cleanup EXIT

fake_bin="$temporary_directory/fake-bin"
runtime_directory="$temporary_directory/csmith runtime"
resource_directory="$temporary_directory/resource dir"
mkdir -p "$fake_bin" "$runtime_directory" "$resource_directory"

cat >"$runtime_directory/csmith.h" <<'EOF'
/* fake Csmith runtime for harness tests */
EOF
printf 'fake-resource-tree\n' >"$resource_directory/manifest.toml"

cat >"$fake_bin/uname" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  -s) echo Linux ;;
  -m) echo x86_64 ;;
  *) echo 'Linux fake-host 1 x86_64 GNU/Linux' ;;
esac
EOF

cat >"$fake_bin/timeout" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
while [[ "${1:-}" == --kill-after=* ]]; do
  shift
done
(($# >= 2))
shift
command_path=$1
if [[ -f "$command_path" && "$command_path" == *reference-*.exe ]]; then
  seed=$(sed -n 's/^# Seed: //p' "$command_path")
  if [[ "${FAKE_REFERENCE_TIMEOUT_SEED:-}" == "$seed" ]]; then
    exit 124
  fi
  if [[ "${FAKE_REFERENCE_PARTIAL_TIMEOUT_SEED:-}" == "$seed" &&
    "$command_path" == *reference-gcc-o0.exe ]]; then
    exit 124
  fi
  if [[ "${FAKE_REFERENCE_KILL_SEED:-}" == "$seed" ]]; then
    exit 137
  fi
fi
exec "$@"
EOF

cat >"$fake_bin/csmith" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --version ]]; then
  echo 'csmith 2.4.0'
  echo 'Git version: 0cdc710'
  exit 0
fi
seed=
expected_options=(
  --no-argc
  --no-float
  --no-packed-struct
  --no-unions
  --no-bitfields
  --no-builtins
  --no-dangling-global-pointers
  --no-return-dead-pointer
  --strict-volatile-rule
  --match-exact-qualifiers
  --safe-math
  --max-funcs 5
  --max-block-depth 4
  --max-block-size 4
  --max-expr-complexity 8
  --max-array-dim 2
  --max-array-len-per-dim 5
)
actual_options=()
while (($#)); do
  if [[ "$1" == --seed ]]; then
    seed=$2
    shift 2
  else
    actual_options+=("$1")
    shift
  fi
done
[[ -n "$seed" ]]
[[ "${#actual_options[@]}" == "${#expected_options[@]}" ]]
for ((index = 0; index < ${#expected_options[@]}; index++)); do
  [[ "${actual_options[$index]}" == "${expected_options[$index]}" ]]
done
if [[ "${FAKE_CSMITH_FAIL_SEED:-}" == "$seed" ]]; then
  echo "synthetic generator failure for seed $seed" >&2
  exit 70
fi
printf '/*\n * Generator: csmith 2.4.0\n * Seed:      %s\n */\n' "$seed"
printf 'int main(void) { return 0; }\n'
EOF

cat >"$fake_bin/gcc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  -dumpmachine)
    echo x86_64-linux-gnu
    exit 0
    ;;
  '-dumpfullversion -dumpversion')
    echo 13.2.0
    exit 0
    ;;
  --version)
    echo 'gcc (Fake GCC) 13.2.0'
    exit 0
    ;;
  '-dM -E -x c /dev/null')
    echo '#define __GNUC__ 13'
    echo '#define __GNUC_MINOR__ 2'
    echo '#define __x86_64__ 1'
    echo '#define __SIZEOF_POINTER__ 8'
    echo '#define __SIZEOF_LONG__ 8'
    echo '#define __LP64__ 1'
    echo '#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__'
    exit 0
    ;;
esac
output=
input_file=
syntax_only=0
while (($#)); do
  case "$1" in
    -o)
      output=$2
      shift 2
      ;;
    -fsyntax-only)
      syntax_only=1
      shift
      ;;
    *.c | *.o)
      input_file=$1
      shift
      ;;
    *)
      shift
      ;;
  esac
done
[[ -n "$input_file" ]]
if ((syntax_only)) && [[ "$input_file" == *csmith-runtime-probe.c ]]; then
  if [[ "${FAKE_RUNTIME_PROBE_FAIL:-}" == 1 ]]; then
    echo 'synthetic broken Csmith runtime' >&2
    exit 1
  fi
  exit 0
fi
seed=$(sed -n 's/.*Seed:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$input_file")
[[ -n "$seed" ]]
if ((syntax_only)); then
  if [[ "${FAKE_SYNTAX_ABNORMAL_SEED:-}" == "$seed" ]]; then
    exit 125
  fi
  if [[ "${FAKE_REJECT_SEED:-}" == "$seed" ||
    "${FAKE_GCC_REJECT_SEED:-}" == "$seed" ]]; then
    echo "synthetic GCC constraint rejection for seed $seed" >&2
    exit 1
  fi
  exit 0
fi
[[ -n "$output" ]]
if [[ "$input_file" == *.o ]]; then
  cp "$input_file" "$output"
  chmod +x "$output"
  exit 0
fi
{
  printf '#!/usr/bin/env bash\n'
  printf '# Seed: %s\n' "$seed"
  printf 'echo %q\n' "checksum = $seed"
} >"$output"
chmod +x "$output"
EOF

cat >"$fake_bin/clang" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  --print-target-triple)
    echo x86_64-pc-linux-gnu
    exit 0
    ;;
  --version)
    echo 'clang version 18.1.0'
    exit 0
    ;;
  '-dM -E -x c /dev/null')
    echo '#define __GNUC__ 4'
    echo '#define __clang__ 1'
    echo '#define __x86_64__ 1'
    echo '#define __SIZEOF_POINTER__ 8'
    echo '#define __SIZEOF_LONG__ 8'
    echo '#define __LP64__ 1'
    echo '#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__'
    exit 0
    ;;
esac
output=
input_file=
syntax_only=0
while (($#)); do
  case "$1" in
    -o)
      output=$2
      shift 2
      ;;
    -fsyntax-only)
      syntax_only=1
      shift
      ;;
    *.c)
      input_file=$1
      shift
      ;;
    *)
      shift
      ;;
  esac
done
[[ -n "$input_file" ]]
if ((syntax_only)) && [[ "$input_file" == *csmith-runtime-probe.c ]]; then
  if [[ "${FAKE_RUNTIME_PROBE_FAIL:-}" == 1 ]]; then
    echo 'synthetic broken Csmith runtime' >&2
    exit 1
  fi
  exit 0
fi
seed=$(sed -n 's/.*Seed:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$input_file")
[[ -n "$seed" ]]
if ((syntax_only)); then
  if [[ "${FAKE_SYNTAX_ABNORMAL_SEED:-}" == "$seed" ]]; then
    exit 125
  fi
  if [[ "${FAKE_REJECT_SEED:-}" == "$seed" ||
    "${FAKE_CLANG_REJECT_SEED:-}" == "$seed" ]]; then
    echo "synthetic Clang constraint rejection for seed $seed" >&2
    exit 1
  fi
  exit 0
fi
[[ -n "$output" ]]
{
  printf '#!/usr/bin/env bash\n'
  printf '# Seed: %s\n' "$seed"
  printf 'echo %q\n' "checksum = $seed"
} >"$output"
chmod +x "$output"
EOF

cat >"$fake_bin/ccc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
expected_cc=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)/gcc
[[ "${CCC_CC:-}" == "$expected_cc" ]]
output=
source_file=
while (($#)); do
  case "$1" in
    -o)
      output=$2
      shift 2
      ;;
    *.c)
      source_file=$1
      shift
      ;;
    *)
      shift
      ;;
  esac
done
[[ -n "$output" && -n "$source_file" ]]
seed=$(sed -n 's/.*Seed:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$source_file")
[[ -n "$seed" ]]
checksum=$seed
if [[ "${FAKE_CCC_MISMATCH_SEED:-}" == "$seed" ]]; then
  checksum=wrong
fi
{
  printf '#!/usr/bin/env bash\n'
  printf '# Seed: %s\n' "$seed"
  printf 'echo %q\n' "checksum = $checksum"
} >"$output"
chmod +x "$output"
EOF

cat >"$fake_bin/objcopy" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --version ]]; then
  echo 'GNU objcopy (Fake Binutils) 2.42'
  exit 0
fi
exit 64
EOF

chmod +x "$fake_bin/uname" "$fake_bin/timeout" "$fake_bin/csmith" \
  "$fake_bin/gcc" "$fake_bin/clang" "$fake_bin/ccc" "$fake_bin/objcopy"

export PATH="$fake_bin:$PATH"

common_arguments=(
  --csmith "$fake_bin/csmith"
  --csmith-runtime "$runtime_directory"
  --allow-unverified-csmith
  --ccc "$fake_bin/ccc"
  --resource-dir "$resource_directory"
  --gcc "$fake_bin/gcc"
  --clang "$fake_bin/clang"
  --generator-timeout 1
  --compile-timeout 1
  --run-timeout 1
)

pass_directory="$temporary_directory/pass results"
pass_output=$(CFLAGS=-fambient CPPFLAGS=-Dambient CXXFLAGS=-fambient \
  LDFLAGS=-Wl,ambient MAKEFILES=/ambient CCC_CC=/ambient/cc \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 3 \
  --start-seed 17 \
  --work-dir "$pass_directory")
[[ "$pass_output" == *"Csmith differential suite: 3/3 completed, 3 passed, 0 failures; 3 attempted"* ]]
[[ "$(grep -c $'\tpass\t' "$pass_directory/summary.tsv")" == 3 ]]
grep -Fq $'17\tpass\t1\t1\t0\tseed 17' "$pass_directory/summary.tsv"
grep -Fq $'19\tpass\t1\t1\t0\tseed 19' "$pass_directory/summary.tsv"
grep -Fq 'generator_options= --no-argc --no-float --no-packed-struct' \
  "$pass_directory/run-config.txt"
grep -Fq 'generator_revision=unverified' "$pass_directory/run-config.txt"
grep -Fq "ccc_native_driver=$fake_bin/gcc" "$pass_directory/run-config.txt"
grep -Fq "ccc_object_copier=$fake_bin/objcopy" "$pass_directory/run-config.txt"
cmp -s "$fake_bin/ccc" "$pass_directory/tool-identities/ccc-executable"
cmp -s "$resource_directory/manifest.toml" \
  "$pass_directory/tool-identities/ccc-resource-dir/manifest.toml"
[[ "$(find "$pass_directory/cases" -name program.c | wc -l | tr -d '[:space:]')" == 3 ]]
! grep -R -E -- '-fno-pie|-no-pie' "$pass_directory/cases"/*/commands.txt

mismatch_directory="$temporary_directory/mismatch results"
set +e
mismatch_output=$(FAKE_CCC_MISMATCH_SEED=23 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --start-seed 23 \
  --work-dir "$mismatch_directory" 2>&1)
mismatch_status=$?
set -e
[[ "$mismatch_status" == 1 ]]
[[ "$mismatch_output" == *"Csmith differential suite: 1/1 completed, 0 passed, 1 failures; 1 attempted"* ]]
grep -Fq $'23\toutput-mismatch\t' "$mismatch_directory/summary.tsv"
mismatch_case=$(find "$mismatch_directory/cases" -mindepth 1 -maxdepth 1 -type d)
[[ -f "$mismatch_case/program.c" ]]
[[ -f "$mismatch_case/commands.txt" ]]
[[ "$(tr -d '[:space:]' <"$mismatch_case/ccc.run.status")" == 0 ]]
! cmp -s "$mismatch_case/reference-gcc-o0.run.stdout" \
  "$mismatch_case/ccc.run.stdout"

generator_directory="$temporary_directory/generator results"
set +e
generator_output=$(FAKE_CSMITH_FAIL_SEED=31 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 3 \
  --start-seed 30 \
  --work-dir "$generator_directory" 2>&1)
generator_status=$?
set -e
[[ "$generator_status" == 1 ]]
[[ "$generator_output" == *"Csmith differential suite: 3/3 completed, 3 passed, 1 failures; 4 attempted"* ]]
grep -Fq $'30\tpass\t' "$generator_directory/summary.tsv"
grep -Fq $'31\tgenerator-failure\t' "$generator_directory/summary.tsv"
grep -Fq $'32\tpass\t' "$generator_directory/summary.tsv"
grep -Fq $'33\tpass\t' "$generator_directory/summary.tsv"

inadmissible_directory="$temporary_directory/inadmissible results"
inadmissible_output=$(FAKE_REJECT_SEED=41 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 2 \
  --start-seed 40 \
  --max-attempts 3 \
  --work-dir "$inadmissible_directory")
[[ "$inadmissible_output" == *"Csmith differential suite: 2/2 completed, 2 passed, 0 failures; 3 attempted"* ]]
grep -Fq $'41\tinadmissible\t0\t0\t0\t' \
  "$inadmissible_directory/summary.tsv"

runtime_failure_directory="$temporary_directory/runtime failure results"
set +e
runtime_failure_output=$(FAKE_RUNTIME_PROBE_FAIL=1 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --work-dir "$runtime_failure_directory" 2>&1)
runtime_failure_status=$?
set -e
[[ "$runtime_failure_status" == 1 ]]
[[ "$runtime_failure_output" == *"Csmith runtime is not valid strict C11"* ]]

syntax_failure_directory="$temporary_directory/syntax failure results"
set +e
syntax_failure_output=$(FAKE_SYNTAX_ABNORMAL_SEED=50 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --start-seed 50 \
  --max-attempts 1 \
  --work-dir "$syntax_failure_directory" 2>&1)
syntax_failure_status=$?
set -e
[[ "$syntax_failure_status" == 1 ]]
grep -Fq $'50\treference-syntax-failure\t0\t0\t1\t' \
  "$syntax_failure_directory/summary.tsv"

timeout_directory="$temporary_directory/timeout results"
timeout_output=$(FAKE_REFERENCE_TIMEOUT_SEED=60 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --start-seed 60 \
  --max-attempts 2 \
  --work-dir "$timeout_directory")
[[ "$timeout_output" == *"1/1 completed, 1 passed, 0 failures; 2 attempted"* ]]
grep -Fq $'60\tinconclusive-timeout\t1\t0\t0\t' \
  "$timeout_directory/summary.tsv"

partial_timeout_directory="$temporary_directory/partial timeout results"
set +e
partial_timeout_output=$(FAKE_REFERENCE_PARTIAL_TIMEOUT_SEED=70 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --start-seed 70 \
  --max-attempts 2 \
  --work-dir "$partial_timeout_directory" 2>&1)
partial_timeout_status=$?
set -e
[[ "$partial_timeout_status" == 1 ]]
grep -Fq $'70\treference-execution-failure\t1\t0\t1\t' \
  "$partial_timeout_directory/summary.tsv"

killed_directory="$temporary_directory/killed results"
set +e
killed_output=$(FAKE_REFERENCE_KILL_SEED=80 \
  "$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --cases 1 \
  --start-seed 80 \
  --max-attempts 1 \
  --work-dir "$killed_directory" 2>&1)
killed_status=$?
set -e
[[ "$killed_status" == 1 ]]
grep -Fq $'80\treference-execution-failure\t1\t0\t1\t' \
  "$killed_directory/summary.tsv"

set +e
invalid_output=$("$script_directory/run.sh" --cases 0 2>&1)
invalid_status=$?
overflow_output=$("$script_directory/run.sh" \
  --cases 18446744073709551617 2>&1)
overflow_status=$?
seed_overflow_output=$("$script_directory/run.sh" \
  --start-seed 18446744073709551617 2>&1)
seed_overflow_status=$?
unpaired_output=$("$script_directory/run.sh" \
  --csmith "$fake_bin/csmith" 2>&1)
unpaired_status=$?
set -e
[[ "$invalid_status" == 2 ]]
[[ "$invalid_output" == *"case count must be a positive integer: 0"* ]]
[[ "$overflow_status" == 2 ]]
[[ "$overflow_output" == *"case count exceeds the safety limit"* ]]
[[ "$seed_overflow_status" == 2 ]]
[[ "$seed_overflow_output" == *"start seed exceeds 4294967295"* ]]
[[ "$unpaired_status" == 2 ]]
[[ "$unpaired_output" == *"--csmith and --csmith-runtime must be provided together"* ]]

nonempty_directory="$temporary_directory/nonempty"
mkdir "$nonempty_directory"
: >"$nonempty_directory/sentinel"
set +e
nonempty_output=$("$script_directory/run.sh" \
  "${common_arguments[@]}" \
  --work-dir "$nonempty_directory" 2>&1)
nonempty_status=$?
set -e
[[ "$nonempty_status" == 2 ]]
[[ "$nonempty_output" == *"work directory must be empty"* ]]
[[ -f "$nonempty_directory/sentinel" ]]

help_output=$("$script_directory/run.sh" --help 2>&1)
[[ "$help_output" == *"--start-seed SEED"* ]]
[[ "$help_output" == *"--csmith-runtime PATH"* ]]
[[ "$help_output" == *"--max-attempts COUNT"* ]]

repository_directory=$(CDPATH= cd -- "$script_directory/../.." && pwd -P)
cdpath_output=$(
  CDPATH="$repository_directory"
  cd "$repository_directory"
  test-corpus/csmith/run.sh --help 2>&1
)
[[ "$cdpath_output" == *"--start-seed SEED"* ]]

leading_dash_output=$(
  CDPATH= cd -- "$temporary_directory"
  "$script_directory/run.sh" \
    "${common_arguments[@]}" \
    --cases 1 \
    --start-seed 90 \
    --work-dir -leading-dash
)
[[ "$leading_dash_output" == *"1/1 completed, 1 passed, 0 failures; 1 attempted"* ]]

grep -Fq 'version = "2.4.0"' "$script_directory/manifest.toml"
grep -Fq 'csmith_version=2.4.0' "$script_directory/profile.sh"
source "$script_directory/profile.sh"
cmp -s <(write_csmith_manifest) "$script_directory/manifest.toml"

echo "Csmith runner tests passed"
