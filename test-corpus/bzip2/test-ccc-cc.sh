#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-bzip2-adapter-test.XXXXXX")
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
export CCC_BZIP2_TARGET=aarch64-unknown-linux-gnu
export CCC_BZIP2_SYSROOT="$temporary_directory/resource-dir"
export TRACE="$temporary_directory/trace"
export CCC_BZIP2_COMMAND_LOG="$temporary_directory/commands"
export CCC_BZIP2_SOURCE_ROOT="$source_directory"
export CCC_BZIP2_SOURCE_LOG="$temporary_directory/sources"

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
: >"$CCC_BZIP2_SOURCE_LOG"
"$script_directory/ccc-cc" -c "$source_directory/a.c" \
  -o "$temporary_directory/a.o"
[[ -f "$temporary_directory/a.o" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 1 ]]
! grep -q '^link ' "$TRACE"
[[ "$(grep -c '^ccc ' "$CCC_BZIP2_COMMAND_LOG")" == 1 ]]
! grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -q -- ' -std='
grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- ' --target=aarch64-unknown-linux-gnu'
grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- " --sysroot=$temporary_directory/resource-dir"
[[ "$(wc -l <"$CCC_BZIP2_SOURCE_LOG" | tr -d '[:space:]')" == 1 ]]
grep -Fxq "$source_directory/a.c" "$CCC_BZIP2_SOURCE_LOG"

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
: >"$CCC_BZIP2_SOURCE_LOG"
"$script_directory/ccc-cc" -std=c99 \
  "$source_directory/a.c" "$source_directory/b.c" \
  -o "$temporary_directory/program" -fpie -fno-PIE \
  -Wl,-pie,-z,now -Xlinker --no-pie -L. -lbz2
[[ -f "$temporary_directory/program" ]]
[[ "$(grep -c '^ccc ' "$TRACE")" == 2 ]]
[[ "$(grep -c '^link ' "$TRACE")" == 1 ]]
! grep '^link ' "$TRACE" | grep -Eq '\.(c|i)( |$)'
! grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Eq -- '-std=|-fpie|-fno-PIE|-L\.|-lbz2'
! grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -Eq -- ' -std=| -fpie| -fno-PIE| -pie( |$)| -no-pie( |$)| --no-pie'
grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- " --sysroot=$temporary_directory/resource-dir"
grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- ' -Wl\,-z\,now'
grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- ' -L.'
grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -q -- ' -lbz2'
[[ "$(grep -c '^ccc ' "$CCC_BZIP2_COMMAND_LOG")" == 2 ]]
! grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -q -- ' -std='
[[ "$(grep -c '^link ' "$CCC_BZIP2_COMMAND_LOG")" == 1 ]]
[[ "$(wc -l <"$CCC_BZIP2_SOURCE_LOG" | tr -d '[:space:]')" == 2 ]]

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
: >"$CCC_BZIP2_SOURCE_LOG"
if FAIL_CCC=1 "$script_directory/ccc-cc" "$source_directory/a.c" \
  -o "$temporary_directory/failed"; then
  echo "ccc-cc test: failed C translation unexpectedly succeeded" >&2
  exit 1
fi
! grep -q '^link ' "$TRACE"
[[ ! -e "$temporary_directory/failed" ]]

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
: >"$CCC_BZIP2_SOURCE_LOG"
if "$script_directory/ccc-cc" "$temporary_directory/a.i" \
  -o "$temporary_directory/preprocessed"; then
  echo "ccc-cc test: preprocessed C input was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
: >"$CCC_BZIP2_SOURCE_LOG"
if "$script_directory/ccc-cc" -c "$temporary_directory/outside.c" \
  -o "$temporary_directory/outside.o"; then
  echo "ccc-cc test: source outside the pinned tree unexpectedly compiled" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -s "$CCC_BZIP2_COMMAND_LOG" ]]
[[ ! -s "$CCC_BZIP2_SOURCE_LOG" ]]

: >"$TRACE"
if "$script_directory/ccc-cc" "@$temporary_directory/inputs.rsp"; then
  echo "ccc-cc test: response file was unexpectedly delegated" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
if env -u CCC_BZIP2_TARGET "$script_directory/ccc-cc" \
  -c "$source_directory/a.c" -o "$temporary_directory/no-target.o"; then
  echo "ccc-cc test: missing target unexpectedly succeeded" >&2
  exit 1
fi
[[ ! -s "$TRACE" ]]
[[ ! -e "$temporary_directory/no-target.o" ]]

: >"$TRACE"
: >"$CCC_BZIP2_COMMAND_LOG"
export CCC_BZIP2_TARGET=aarch64-apple-darwin
export CCC_BZIP2_SDKROOT="$temporary_directory/resource-dir"
export CCC_BZIP2_DEPLOYMENT_TARGET=11.0
"$script_directory/ccc-cc" -c "$source_directory/a.c" \
  -o "$temporary_directory/darwin.o"
grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- ' --target=aarch64-apple-darwin'
grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- " -isysroot $temporary_directory/resource-dir"
grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Fq -- ' -mmacosx-version-min=11.0'
[[ -f "$temporary_directory/darwin.o" ]]

echo "bzip2 ccc-cc adapter tests passed"
