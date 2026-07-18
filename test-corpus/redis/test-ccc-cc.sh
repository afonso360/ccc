#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-redis-adapter-test.XXXXXX")
temporary_directory=$(cd "$temporary_directory" && pwd -P)
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source_directory="$temporary_directory/source tree"
mkdir "$temporary_directory/resource-dir" "$source_directory"
: >"$source_directory/a.c"
: >"$source_directory/b.c"
: >"$temporary_directory/outside.c"

cat >"$temporary_directory/fake-ccc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$CCC_CC" == "$EXPECTED_CCC_CC" ]]
printf 'ccc' >>"$TRACE"
printf ' %q' "$@" >>"$TRACE"
printf '\n' >>"$TRACE"
output=
frontend=false
while (($#)); do
  if [[ "$1" == -o ]]; then
    output=$2
    shift 2
  elif [[ "$1" == -E ]]; then
    frontend=true
    shift
  else
    shift
  fi
done
if ! $frontend && [[ "${FAIL_CCC:-0}" != 0 ]]; then
  exit 42
fi
if $frontend; then
  printf '%s\n' 'int redis_preprocessing_capture;'
fi
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
export CCC_REDIS_COMMAND_LOG="$temporary_directory/commands"
export CCC_REDIS_SOURCE_ROOT="$source_directory"
export CCC_REDIS_SOURCE_LOG="$temporary_directory/sources"
export CCC_REDIS_PREPROCESS_DIR="$temporary_directory/preprocessed"

: >"$TRACE"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"
"$script_directory/ccc-cc" -std=c99 -ggdb -pedantic -fPIC \
  -DREDIS_TEST_FLAG=1 -MMD -MF "$temporary_directory/a.d" \
  -MT "$temporary_directory/redis-a.o" \
  -c "$source_directory/a.c" -o "$temporary_directory/a.o"
[[ -f "$temporary_directory/a.o" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 2 ]]
! grep -q '^link ' "$TRACE"
grep -q -- ' -std=gnu11' "$TRACE"
! grep -Eq -- '-std=c99|-ggdb|-pedantic|-fPIC' "$TRACE"
grep -q -- ' -DREDIS_TEST_FLAG=1' "$TRACE"
[[ "$(grep -c '^ccc ' "$CCC_REDIS_COMMAND_LOG")" == 1 ]]
[[ "$(grep -c '^preprocess ' "$CCC_REDIS_COMMAND_LOG")" == 1 ]]
grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -Fq -- ' -MMD'
grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -Fq -- " -MF $temporary_directory/a.d"
grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -Fq -- " -MT $temporary_directory/redis-a.o"
if grep '^preprocess ' "$CCC_REDIS_COMMAND_LOG" | grep -Eq -- ' -MMD| -MF( |$)| -MT( |$)'; then
  echo "ccc-cc test: preprocessing capture retained dependency side effects" >&2
  exit 1
fi
grep -Fxq "$source_directory/a.c" "$CCC_REDIS_SOURCE_LOG"
[[ -s "$CCC_REDIS_PREPROCESS_DIR/a.c.i" ]]

: >"$TRACE"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"
"$script_directory/ccc-cc" -std=gnu11 \
  "$source_directory/a.c" "$source_directory/b.c" \
  -o "$temporary_directory/program" -no-pie -L. -lhiredis
[[ -f "$temporary_directory/program" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 4 ]]
[[ "$(grep -c '^link ' "$TRACE")" == 1 ]]
! grep '^link ' "$TRACE" | grep -Eq '\.(c|i)( |$)'
! grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -Eq -- '-no-pie|-L\.|-lhiredis'
grep '^link ' "$CCC_REDIS_COMMAND_LOG" | grep -q -- ' -no-pie'
grep '^link ' "$CCC_REDIS_COMMAND_LOG" | grep -Fq -- ' -L.'
grep '^link ' "$CCC_REDIS_COMMAND_LOG" | grep -q -- ' -lhiredis'
[[ "$(grep -c '^ccc ' "$CCC_REDIS_COMMAND_LOG")" == 2 ]]
[[ "$(grep -c '^preprocess ' "$CCC_REDIS_COMMAND_LOG")" == 2 ]]
[[ "$(grep -c '^link ' "$CCC_REDIS_COMMAND_LOG")" == 1 ]]
[[ "$(wc -l <"$CCC_REDIS_SOURCE_LOG" | tr -d '[:space:]')" == 2 ]]

: >"$TRACE"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"
if FAIL_CCC=1 "$script_directory/ccc-cc" "$source_directory/a.c" \
  -o "$temporary_directory/failed"; then
  echo "ccc-cc test: failed C translation unexpectedly succeeded" >&2
  exit 1
fi
! grep -q '^link ' "$TRACE"
[[ ! -e "$temporary_directory/failed" ]]

: >"$TRACE"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"
if "$script_directory/ccc-cc" "$temporary_directory/a.i" \
  -o "$temporary_directory/preprocessed"; then
  echo "ccc-cc test: preprocessed C input was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"
if "$script_directory/ccc-cc" -c "$temporary_directory/outside.c" \
  -o "$temporary_directory/outside.o"; then
  echo "ccc-cc test: source outside the pinned tree unexpectedly compiled" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_REDIS_COMMAND_LOG" ]]
[[ ! -s "$CCC_REDIS_SOURCE_LOG" ]]

: >"$TRACE"
if "$script_directory/ccc-cc" "@$temporary_directory/inputs.rsp"; then
  echo "ccc-cc test: response file was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

echo "Redis ccc-cc adapter tests passed"
