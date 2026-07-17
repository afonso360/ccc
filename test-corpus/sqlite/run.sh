#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_directory/../.." && pwd)
manifest="$script_directory/manifest.toml"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$manifest"
}

manifest_integer() {
  sed -n "s/^$1 = \([0-9][0-9]*\)$/\1/p" "$manifest"
}

usage() {
  echo "usage: $0 [--archive PATH] [--work-dir PATH] [--jobs COUNT]" >&2
}

die() {
  echo "$*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "required tool is not available: $1"
}

absolute_directory() {
  [[ -d "$1" ]] || die "directory does not exist: $1"
  (cd "$1" && pwd -P)
}

absolute_file() {
  [[ -f "$1" ]] || die "file does not exist: $1"
  printf '%s/%s\n' "$(cd "$(dirname "$1")" && pwd -P)" "$(basename "$1")"
}

resolve_executable() {
  local executable=$1
  local resolved
  resolved=$(command -v "$executable" 2>/dev/null) || die "executable is not available: $executable"
  [[ -x "$resolved" ]] || die "file is not executable: $resolved"
  if [[ "$resolved" != /* ]]; then
    resolved="$(pwd -P)/$resolved"
  fi
  printf '%s\n' "$resolved"
}

archive=${SQLITE_ARCHIVE:-}
work_directory=${SQLITE_WORK_DIR:-}
jobs=${SQLITE_BUILD_JOBS:-2}

while (($#)); do
  case "$1" in
    --archive)
      (($# >= 2)) || {
        usage
        exit 2
      }
      archive=$2
      shift 2
      ;;
    --work-dir)
      (($# >= 2)) || {
        usage
        exit 2
      }
      work_directory=$2
      shift 2
      ;;
    --jobs)
      (($# >= 2)) || {
        usage
        exit 2
      }
      jobs=$2
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || {
  echo "SQLite build job count must be a positive integer: $jobs" >&2
  exit 2
}

for tool in awk find grep id make mktemp openssl sed tee tr unzip wc; do
  require_tool "$tool"
done

if [[ "$(id -u)" == 0 ]]; then
  die "SQLite's Tcl suite must run as a non-root user so permission tests are meaningful"
fi

version=$(manifest_string version)
origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_bytes=$(manifest_integer archive_bytes)
expected_sha256=$(manifest_string archive_sha256)
expected_sha3=$(manifest_string archive_sha3_256)
expected_source_id=$(manifest_string source_id)
expected_generated_sha3=$(manifest_string generated_sqlite3_sha3_256)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-sqlite-$version.XXXXXX")
else
  if [[ -e "$work_directory" ]] && [[ ! -d "$work_directory" ]]; then
    echo "SQLite work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] && [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "SQLite work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

if [[ -z "$archive" ]]; then
  cache_directory=${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/sqlite
  mkdir -p "$cache_directory"
  archive="$cache_directory/$archive_name"
  if [[ ! -f "$archive" ]]; then
    require_tool curl
    partial_archive="$archive.part.$$"
    trap 'rm -f -- "$partial_archive"' EXIT
    curl --fail --location --output "$partial_archive" "$origin"
    mv "$partial_archive" "$archive"
    trap - EXIT
  fi
fi
archive=$(absolute_file "$archive")

actual_bytes=$(wc -c <"$archive" | tr -d '[:space:]')
actual_sha256=$(openssl dgst -sha256 "$archive" | awk '{print $NF}')
actual_sha3=$(openssl dgst -sha3-256 "$archive" | awk '{print $NF}')
[[ "$actual_bytes" == "$expected_bytes" ]] || {
  echo "SQLite archive size mismatch: expected $expected_bytes, found $actual_bytes" >&2
  exit 1
}
[[ "$actual_sha256" == "$expected_sha256" ]] || {
  echo "SQLite archive SHA-256 mismatch" >&2
  exit 1
}
[[ "$actual_sha3" == "$expected_sha3" ]] || {
  echo "SQLite archive SHA3-256 mismatch" >&2
  exit 1
}

source_parent="$work_directory/source"
mkdir -p "$source_parent"
unzip -q "$archive" -d "$source_parent"
source_directory="$source_parent/${archive_name%.zip}"
[[ "$(tr -d '\r\n' <"$source_directory/VERSION")" == "$version" ]] || {
  echo "SQLite VERSION does not match the corpus pin" >&2
  exit 1
}
source_hash=${expected_source_id##* }
[[ "$(tr -d '\r\n' <"$source_directory/manifest.uuid")" == "$source_hash" ]] || {
  echo "SQLite manifest.uuid does not match the corpus pin" >&2
  exit 1
}

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_LINK_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
export CCC CCC_RESOURCE_DIR CCC_LINK_CC
export CCC_SQLITE_COMMAND_LOG="$work_directory/build/compile-commands.txt"

# Ambient build flags or configure cache hooks would change the selected C
# surface without changing the checked-in corpus contract.
unset BCC BUILD_CC CFLAGS CONFIG_SITE CPPFLAGS LDFLAGS LIBS
unset MAKEFLAGS MAKEOVERRIDES MFLAGS OPTS TCC TESTOPTS

build_directory="$work_directory/build"
mkdir -p "$build_directory"
cd "$build_directory"
: >"$CCC_SQLITE_COMMAND_LOG"

# glibc exposes isnan as a type-generic macro whose expansion mentions
# __isnanl even for a double operand. CCC deliberately has no long-double ABI,
# so the configure link probe is not a sufficient test of whether that macro is
# usable. Selecting SQLite's own binary64 test keeps the configured surface
# aligned with the compiler's advertised ABI without patching upstream source.
ac_cv_func_isnan=no \
  CC="$script_directory/ccc-cc" \
  "$source_directory/configure" \
  --disable-shared \
  --disable-readline \
  --disable-load-extension

grep -Fxq 'ac_cv_func_isnan=no' config.log ||
  die "SQLite configure did not honor the pinned isnan capability decision"
grep -Fq '/* #undef HAVE_ISNAN */' sqlite_cfg.h ||
  die "SQLite configure unexpectedly enabled the host isnan interface"

"$script_directory/ccc-cc" -std=gnu11 -dM -E \
  "$script_directory/predicate-probe.c" >effective-macros.txt
"$script_directory/ccc-cc" -std=gnu11 -P -E \
  "$script_directory/predicate-probe.c" >predicate-probe.txt

grep -Fxq '#define __GNUC__ 4' effective-macros.txt ||
  die "CCC does not advertise the pinned __GNUC__ value"
grep -Fxq '#define __GNUC_MINOR__ 2' effective-macros.txt ||
  die "CCC does not advertise the pinned __GNUC_MINOR__ value"
grep -Fxq '#define __GNUC_PATCHLEVEL__ 1' effective-macros.txt ||
  die "CCC does not advertise the pinned __GNUC_PATCHLEVEL__ value"
grep -Fxq 'selected_builtin=__sync_synchronize' predicate-probe.txt ||
  die "CCC does not select SQLite's required full-barrier builtin"
for unexpected in \
  selected_builtin=__atomic_load_n \
  selected_builtin=__atomic_store_n \
  selected_builtin=__builtin_bswap32 \
  selected_builtin=__builtin_add_overflow \
  selected_builtin=__builtin_clzll; do
  if grep -Fxq "$unexpected" predicate-probe.txt; then
    die "CCC unexpectedly selects a compiler path outside the pinned inventory: $unexpected"
  fi
done

make -j"$jobs" BCC="$CCC_LINK_CC -g" sqlite3.c
actual_generated_sha3=$(openssl dgst -sha3-256 sqlite3.c | awk '{print $NF}')
[[ "$actual_generated_sha3" == "$expected_generated_sha3" ]] || {
  echo "generated sqlite3.c SHA3-256 mismatch" >&2
  exit 1
}
grep -Fq "$expected_source_id" sqlite3.c || {
  echo "generated sqlite3.c source ID mismatch" >&2
  exit 1
}

{
  grep '^selected_builtin=' predicate-probe.txt
  printf '%s\n' \
    'inline_assembly=none' \
    'wide_integer_use=none' \
    'variable_length_array_object=none' \
    'statement_expression=none' \
    'computed_goto=none'
} >capability-inventory.txt

make -j1 BCC="$CCC_LINK_CC -g" CC="$script_directory/ccc-cc" tcltest \
  2>&1 | tee test-run.log

[[ -x testfixture ]] || {
  echo "SQLite testfixture was not produced" >&2
  exit 1
}
[[ -f test-out.txt ]] || {
  echo "SQLite test output was not produced" >&2
  exit 1
}

printf 'SQLite %s veryquick artifacts: %s\n' "$version" "$build_directory"
