#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-sqlite-adapter-test.XXXXXX")
temporary_directory=$(cd "$temporary_directory" && pwd -P)
cleanup() {
  if [[ -n "$temporary_directory" ]] && [[ -d "$temporary_directory" ]]; then
    rm -rf -- "$temporary_directory"
  fi
}
trap cleanup EXIT

source_directory="$temporary_directory/source tree"
generated_directory="$temporary_directory/generated tree"
generated_source="$generated_directory/sqlite3.c"
other_generated_source="$generated_directory/sqlite3_analyzer.c"
mkdir -p "$temporary_directory/resource-dir" \
  "$source_directory/src" "$generated_directory"
: >"$source_directory/src/a.c"
: >"$source_directory/src/b.c"
: >"$generated_source"
: >"$other_generated_source"
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
export CCC_SQLITE_COMMAND_LOG="$temporary_directory/commands"
export CCC_SQLITE_SOURCE_ROOT="$source_directory"
export CCC_SQLITE_GENERATED_SOURCE_ROOT="$generated_directory"
export CCC_SQLITE_SOURCE_LOG="$temporary_directory/sources"
export CCC_SQLITE_LANGUAGE_MODE_LOG="$temporary_directory/language-modes"
export CCC_SQLITE_FUZZCHECK_HWTIME_FALLBACK=1

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
"$script_directory/ccc-cc" -c "$source_directory/src/a.c" \
  -o "$temporary_directory/a.o"
[[ -f "$temporary_directory/a.o" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 1 ]]
! grep -q '^link ' "$TRACE"
[[ "$(grep -c '^ccc ' "$CCC_SQLITE_COMMAND_LOG")" == 1 ]]
[[ "$(wc -l <"$CCC_SQLITE_SOURCE_LOG" | tr -d '[:space:]')" == 1 ]]
grep -Fxq "$source_directory/src/a.c" "$CCC_SQLITE_SOURCE_LOG"
grep -Eq '^gnu11 ordinary strict-ansi=absent ' "$CCC_SQLITE_LANGUAGE_MODE_LOG"
! grep '^ccc ' "$CCC_SQLITE_COMMAND_LOG" | grep -q -- ' -std='

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
"$script_directory/ccc-cc" -std=c11 -std=gnu11 -U__STRICT_ANSI__ \
  -DSQLITE_OSS_FUZZ \
  -c "$generated_source" -o "$temporary_directory/fuzzcheck-sqlite3.o"
[[ -f "$temporary_directory/fuzzcheck-sqlite3.o" ]]
grep -Eq '^gnu11 fuzzcheck-amalgamation strict-ansi=defined ' \
  "$CCC_SQLITE_LANGUAGE_MODE_LOG"
awk '
  $1 == "ccc" {
    for( i=1; i<=NF; i++ ) {
      if( $i ~ /^-std=/ ) standard=$i
      if( $i ~ /^-D__STRICT_ANSI__(=|$)/ ) predicate="defined"
      if( $i == "-U__STRICT_ANSI__" ) predicate="absent"
    }
  }
  END {
    exit standard == "-std=gnu11" && predicate == "defined" ? 0 : 1
  }
' "$CCC_SQLITE_COMMAND_LOG"

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
if "$script_directory/ccc-cc" -std=c11 -DSQLITE_OSS_FUZZ \
  -c "$source_directory/src/a.c" -o "$temporary_directory/wrong-fuzzcheck.o"; then
  echo "ccc-cc test: fuzzcheck support input unexpectedly used strict C11" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_SQLITE_LANGUAGE_MODE_LOG" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
"$script_directory/ccc-cc" -DSQLITE_OSS_FUZZ \
  -c "$source_directory/src/a.c" -o "$temporary_directory/fuzzcheck-support.o"
[[ -f "$temporary_directory/fuzzcheck-support.o" ]]
grep -Eq '^gnu11 fuzzcheck-support strict-ansi=absent ' \
  "$CCC_SQLITE_LANGUAGE_MODE_LOG"

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
if "$script_directory/ccc-cc" -std=c11 -c "$source_directory/src/a.c" \
  -o "$temporary_directory/wrong-ordinary.o"; then
  echo "ccc-cc test: ordinary translation unexpectedly used strict C11" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_SQLITE_LANGUAGE_MODE_LOG" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"
if "$script_directory/ccc-cc" -D__STRICT_ANSI__=1 \
  -c "$source_directory/src/a.c" -o "$temporary_directory/wrong-predicate.o"; then
  echo "ccc-cc test: hwtime predicate override leaked to an ordinary input" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_SQLITE_LANGUAGE_MODE_LOG" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
"$script_directory/ccc-cc" "$source_directory/src/a.c" \
  "$source_directory/src/b.c" "$generated_source" \
  -o "$temporary_directory/program" -Wl,-E -ldl -lm
[[ -f "$temporary_directory/program" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 3 ]]
[[ "$(grep -c '^link ' "$TRACE")" == 1 ]]
! grep '^link ' "$TRACE" | grep -Eq '\.(c|i)( |$)'
! grep '^ccc ' "$CCC_SQLITE_COMMAND_LOG" | grep -Eq -- '-no-pie|-Wl,-E|-ldl|-lm'
! grep '^link ' "$CCC_SQLITE_COMMAND_LOG" | grep -q -- ' -no-pie'
grep '^link ' "$CCC_SQLITE_COMMAND_LOG" | grep -Fq -- ' -Wl\,-E'
grep '^link ' "$CCC_SQLITE_COMMAND_LOG" | grep -q -- ' -ldl'
[[ "$(grep -c '^ccc ' "$CCC_SQLITE_COMMAND_LOG")" == 3 ]]
[[ "$(grep -c '^link ' "$CCC_SQLITE_COMMAND_LOG")" == 1 ]]
[[ "$(wc -l <"$CCC_SQLITE_SOURCE_LOG" | tr -d '[:space:]')" == 3 ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
if FAIL_CCC=1 "$script_directory/ccc-cc" "$source_directory/src/a.c" \
  -o "$temporary_directory/failed"; then
  echo "ccc-cc test: failed C translation unexpectedly succeeded" >&2
  exit 1
fi
! grep -q '^link ' "$TRACE"
[[ ! -e "$temporary_directory/failed" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
if "$script_directory/ccc-cc" "$temporary_directory/a.i" \
  -o "$temporary_directory/preprocessed"; then
  echo "ccc-cc test: preprocessed C input was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
if "$script_directory/ccc-cc" -c "$temporary_directory/outside.c" \
  -o "$temporary_directory/outside.o"; then
  echo "ccc-cc test: source outside the pinned tree unexpectedly compiled" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_SQLITE_COMMAND_LOG" ]]
[[ ! -s "$CCC_SQLITE_SOURCE_LOG" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
"$script_directory/ccc-cc" -c "$other_generated_source" \
  -o "$temporary_directory/generated.o"
[[ -f "$temporary_directory/generated.o" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 1 ]]
grep -Fxq "$other_generated_source" "$CCC_SQLITE_SOURCE_LOG"

: >"$TRACE"
if "$script_directory/ccc-cc" "@$temporary_directory/inputs.rsp"; then
  echo "ccc-cc test: response file was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
pids=()
for index in 0 1 2 3 4 5 6 7; do
  "$script_directory/ccc-cc" -c "$source_directory/src/a.c" \
    -o "$temporary_directory/parallel-$index.o" &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
[[ "$(grep -c '^ccc ' "$CCC_SQLITE_COMMAND_LOG")" == 8 ]]
[[ "$(grep -c -v '^ccc ' "$CCC_SQLITE_COMMAND_LOG" || true)" == 0 ]]
[[ "$(wc -l <"$CCC_SQLITE_SOURCE_LOG" | tr -d '[:space:]')" == 8 ]]

echo "ccc-cc adapter tests passed"
