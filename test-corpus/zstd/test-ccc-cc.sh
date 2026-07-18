#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-zstd-adapter-test.XXXXXX")
temporary_directory=$(cd "$temporary_directory" && pwd -P)
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source_directory="$temporary_directory/source tree"
mkdir -p "$temporary_directory/resource-dir" "$source_directory/lib" "$source_directory/tests"
: >"$source_directory/lib/a.c"
: >"$source_directory/tests/b.c"
: >"$source_directory/lib/deps.h"
printf '#include <pthread.h>\nint main(void) { return 0; }' >"$source_directory/have_pthread.c"
: >"$temporary_directory/outside.c"

cat >"$temporary_directory/fake-ccc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$CCC_CC" == "$EXPECTED_CCC_CC" ]]
printf 'ccc' >>"$TRACE"
printf ' %q' "$@" >>"$TRACE"
printf '\n' >>"$TRACE"
[[ "${FAIL_CCC:-0}" == 0 ]] || exit 42
output=
while (($#)); do
  if [[ "$1" == -o ]]; then
    output=$2
    shift 2
  else
    shift
  fi
done
[[ -z "$output" ]] || : >"$output"
EOF

cat >"$temporary_directory/fake-link" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'link' >>"$TRACE"
printf ' %q' "$@" >>"$TRACE"
printf '\n' >>"$TRACE"
output=a.out
while (($#)); do
  if [[ "$1" == -o ]]; then
    output=$2
    shift 2
  else
    shift
  fi
done
: >"$output"
EOF
chmod +x "$temporary_directory/fake-ccc" "$temporary_directory/fake-link"

export CCC="$temporary_directory/fake-ccc"
export CCC_RESOURCE_DIR="$temporary_directory/resource-dir"
export CCC_LINK_CC="$temporary_directory/fake-link"
export EXPECTED_CCC_CC="$CCC_LINK_CC"
export CCC_CC="$temporary_directory/ambient-driver-must-not-win"
export TRACE="$temporary_directory/trace"
export CCC_ZSTD_COMMAND_LOG="$temporary_directory/commands"
export CCC_ZSTD_SOURCE_ROOT="$source_directory"
export CCC_ZSTD_SOURCE_LOG="$temporary_directory/sources"
printf -v quoted_dependency_override '%q' "$source_directory/lib/deps.h"
export CCC_ZSTD_PTHREAD_PROBE="$source_directory/have_pthread.c"
export CCC_ZSTD_PTHREAD_PROBE_SHA256=ec0d733a96cf302e7a028680433e75663e240ea080c0a46e68b19b30c8cf9387
export CCC_ZSTD_PROBE_HASH_LOG="$temporary_directory/probe-hashes"

: >"$TRACE"
: >"$CCC_ZSTD_COMMAND_LOG"
: >"$CCC_ZSTD_SOURCE_LOG"
"$script_directory/ccc-cc" -std=gnu11 -c "$source_directory/lib/a.c" \
  -MMD -MP -MF "$temporary_directory/a.d" -MT dependency-target.o \
  -o "$temporary_directory/a.o"
[[ -f "$temporary_directory/a.o" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 1 ]]
! grep -q '^link ' "$TRACE"
grep -Fxq "$source_directory/lib/a.c" "$CCC_ZSTD_SOURCE_LOG"
grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fq -- ' -MMD -MP -MF'
grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fq -- ' -MT dependency-target.o'

: >"$TRACE"
: >"$CCC_ZSTD_COMMAND_LOG"
: >"$CCC_ZSTD_SOURCE_LOG"
: >"$CCC_ZSTD_PROBE_HASH_LOG"
"$script_directory/ccc-cc" -std=gnu11 -c "$source_directory/have_pthread.c" \
  -o "$temporary_directory/have_pthread.o"
grep -Fxq \
  "$CCC_ZSTD_PTHREAD_PROBE_SHA256  $CCC_ZSTD_PTHREAD_PROBE" \
  "$CCC_ZSTD_PROBE_HASH_LOG"

printf '#include <pthread.h>\nint main(void) { return 1; }' >"$CCC_ZSTD_PTHREAD_PROBE"
: >"$TRACE"
set +e
probe_failure_output=$("$script_directory/ccc-cc" -std=gnu11 -c \
  "$CCC_ZSTD_PTHREAD_PROBE" -o "$temporary_directory/rejected-probe.o" 2>&1)
probe_failure_status=$?
set -e
[[ "$probe_failure_status" == 2 ]]
[[ "$probe_failure_output" == *"does not match the pinned source"* ]]
[[ ! -s "$TRACE" ]]
[[ ! -e "$temporary_directory/rejected-probe.o" ]]

: >"$TRACE"
: >"$CCC_ZSTD_COMMAND_LOG"
: >"$CCC_ZSTD_SOURCE_LOG"
"$script_directory/ccc-cc" "$source_directory/lib/a.c" \
  "$source_directory/tests/b.c" -o "$temporary_directory/program" \
  -I"$source_directory/lib" \
  -include "$source_directory/lib/deps.h" \
  -pthread -no-pie -Wl,-z,now -z relro -lm
[[ -f "$temporary_directory/program" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 2 ]]
[[ "$(grep -c '^link ' "$TRACE")" == 1 ]]
! grep '^link ' "$TRACE" | grep -Eq '\.(c|i)( |$)'
! grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -Eq -- '-pthread|-no-pie|-Wl,|-z relro|-lm'
[[ "$(grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fc -- " -include $quoted_dependency_override")" == 2 ]]
grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -q -- ' -pthread'
grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -q -- ' -no-pie'
grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fq -- ' -Wl\,-z\,now'
! grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fq -- "$source_directory/lib/deps.h"
! grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -Fq -- "$source_directory/lib"
[[ "$(wc -l <"$CCC_ZSTD_SOURCE_LOG" | tr -d '[:space:]')" == 2 ]]

: >"$TRACE"
: >"$CCC_ZSTD_COMMAND_LOG"
: >"$CCC_ZSTD_SOURCE_LOG"
if FAIL_CCC=1 "$script_directory/ccc-cc" "$source_directory/lib/a.c" \
  -o "$temporary_directory/failed"; then
  echo "ccc-cc test: failed C translation unexpectedly succeeded" >&2
  exit 1
fi
! grep -q '^link ' "$TRACE"
[[ ! -e "$temporary_directory/failed" ]]

: >"$TRACE"
if "$script_directory/ccc-cc" -c "$temporary_directory/outside.c" \
  -o "$temporary_directory/outside.o"; then
  echo "ccc-cc test: source outside the pinned tree unexpectedly compiled" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
if "$script_directory/ccc-cc" -c "$temporary_directory/input.S" \
  -o "$temporary_directory/input.o"; then
  echo "ccc-cc test: assembly input was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

version_output=$("$script_directory/ccc-cc" --version)
[[ "$version_output" == *"ccc zstd corpus adapter"* ]]
[[ "$("$script_directory/ccc-cc" -dumpmachine)" == x86_64-unknown-linux-gnu ]]

: >"$TRACE"
if "$script_directory/ccc-cc" "@$temporary_directory/inputs.rsp"; then
  echo "ccc-cc test: response file was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

echo "zstd ccc-cc adapter tests passed"
