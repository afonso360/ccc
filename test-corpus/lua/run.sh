#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_directory/../.." && pwd)
manifest="$script_directory/manifest.toml"
source "$repository/test-corpus/adapter-environment.sh"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$manifest"
}

manifest_integer() {
  sed -n "s/^$1 = \([0-9][0-9]*\)$/\1/p" "$manifest"
}

usage() {
  echo "usage: $0 [--source-archive PATH] [--test-archive PATH] [--work-dir PATH] [--jobs COUNT]" >&2
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

verify_archive() {
  local label=$1
  local path=$2
  local expected_bytes=$3
  local expected_sha256=$4
  local expected_sha3=$5
  local actual_bytes actual_sha256 actual_sha3

  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$(openssl dgst -sha256 "$path" | awk '{print $NF}')
  actual_sha3=$(openssl dgst -sha3-256 "$path" | awk '{print $NF}')
  [[ "$actual_bytes" == "$expected_bytes" ]] ||
    die "$label archive size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] ||
    die "$label archive SHA-256 mismatch"
  [[ "$actual_sha3" == "$expected_sha3" ]] ||
    die "$label archive SHA3-256 mismatch"
}

download_archive() {
  local origin=$1
  local destination=$2
  local partial="$destination.part.$$"
  if [[ ! -f "$destination" ]]; then
    require_tool curl
    if ! curl --fail --location --output "$partial" "$origin"; then
      rm -f -- "$partial"
      return 1
    fi
    mv "$partial" "$destination"
  fi
}

source_archive=${LUA_SOURCE_ARCHIVE:-}
test_archive=${LUA_TEST_ARCHIVE:-}
work_directory=${LUA_WORK_DIR:-}
jobs=${LUA_BUILD_JOBS:-2}

while (($#)); do
  case "$1" in
    --source-archive)
      (($# >= 2)) || {
        usage
        exit 2
      }
      source_archive=$2
      shift 2
      ;;
    --test-archive)
      (($# >= 2)) || {
        usage
        exit 2
      }
      test_archive=$2
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
  echo "Lua build job count must be a positive integer: $jobs" >&2
  exit 2
}

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
  die "Lua execution validation requires x86-64 Linux"

for tool in ar awk basename cmp dirname find gcc grep make mkdir mktemp mv openssl ranlib readelf rm sed sort tar tee tr uname wc; do
  require_tool "$tool"
done

version=$(manifest_string version)
source_origin=$(manifest_string origin)
source_archive_name=$(manifest_string archive)
source_expected_bytes=$(manifest_integer archive_bytes)
source_expected_sha256=$(manifest_string archive_sha256)
source_expected_sha3=$(manifest_string archive_sha3_256)
test_origin=$(manifest_string test_origin)
test_archive_name=$(manifest_string test_archive)
test_expected_bytes=$(manifest_integer test_archive_bytes)
test_expected_sha256=$(manifest_string test_archive_sha256)
test_expected_sha3=$(manifest_string test_archive_sha3_256)
expected_translation_units=$(manifest_integer source_translation_units)
success_marker=$(manifest_string success_marker)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-lua-$version.XXXXXX")
else
  if [[ -e "$work_directory" ]] && [[ ! -d "$work_directory" ]]; then
    echo "Lua work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "Lua work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

cache_directory=${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/lua
if [[ -z "$source_archive" || -z "$test_archive" ]]; then
  mkdir -p "$cache_directory"
fi
if [[ -z "$source_archive" ]]; then
  source_archive="$cache_directory/$source_archive_name"
  download_archive "$source_origin" "$source_archive"
fi
if [[ -z "$test_archive" ]]; then
  test_archive="$cache_directory/$test_archive_name"
  download_archive "$test_origin" "$test_archive"
fi
source_archive=$(absolute_file "$source_archive")
test_archive=$(absolute_file "$test_archive")

verify_archive "Lua source" "$source_archive" "$source_expected_bytes" \
  "$source_expected_sha256" "$source_expected_sha3"
verify_archive "Lua test" "$test_archive" "$test_expected_bytes" \
  "$test_expected_sha256" "$test_expected_sha3"

source_parent="$work_directory/source"
test_parent="$work_directory/tests"
mkdir -p "$source_parent" "$test_parent"
tar -xzf "$source_archive" -C "$source_parent"
tar -xzf "$test_archive" -C "$test_parent"
source_directory="$source_parent/lua-$version"
test_directory="$test_parent/lua-$version-tests"
[[ -d "$source_directory/src" ]] || die "Lua source archive has an unexpected layout"
[[ -f "$test_directory/all.lua" ]] || die "Lua test archive has an unexpected layout"

grep -Eq '^#define LUA_VERSION_MAJOR_N[[:space:]]+5$' "$source_directory/src/lua.h" ||
  die "Lua source major version does not match the corpus pin"
grep -Eq '^#define LUA_VERSION_MINOR_N[[:space:]]+5$' "$source_directory/src/lua.h" ||
  die "Lua source minor version does not match the corpus pin"
grep -Eq '^#define LUA_VERSION_RELEASE_N[[:space:]]+0$' "$source_directory/src/lua.h" ||
  die "Lua source release version does not match the corpus pin"
grep -Fq 'Permission is hereby granted, free of charge' "$source_directory/src/lua.h" ||
  die "Lua source license text is missing"
grep -Fq 'local version = "Lua 5.5"' "$test_directory/all.lua" ||
  die "Lua test suite version does not match the source pin"

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_LINK_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
export CCC CCC_RESOURCE_DIR CCC_LINK_CC
export CCC_LUA_COMMAND_LOG="$work_directory/compile-commands.txt"

record_native_gcc_driver \
  Lua "$CCC_LINK_CC" \
  "$work_directory/link-driver-identity.txt" \
  "$work_directory/link-driver-macros.txt"

# Ambient flags can change both the selected source paths and the ABI used by
# Lua and its embedding interface, so the corpus owns the complete build input.
clear_ambient_make_injection
unset CFLAGS CPPFLAGS LDFLAGS LIBS
unset MYCFLAGS MYLDFLAGS MYLIBS MYOBJS SYSCFLAGS SYSLDFLAGS SYSLIBS

: >"$CCC_LUA_COMMAND_LOG"
"$script_directory/ccc-cc" -dM -E \
  "$script_directory/predicate-probe.c" >"$work_directory/effective-macros.txt"
"$script_directory/ccc-cc" -P -E \
  "$script_directory/predicate-probe.c" >"$work_directory/predicate-probe.txt"
"$script_directory/ccc-cc" -P -E \
  "$script_directory/hosted-probe.c" | grep '^selected_' \
  >"$work_directory/hosted-probe.txt"

grep -Fxq '#define __GNUC__ 4' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC__ value"
grep -Fxq '#define __GNUC_MINOR__ 2' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_MINOR__ value"
grep -Fxq '#define __GNUC_PATCHLEVEL__ 1' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_PATCHLEVEL__ value"
grep -Fxq '#define __DBL_MANT_DIG__ 53' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the binary64 mantissa width required by Lua"
grep -Fxq '#define __DBL_MAX_10_EXP__ 308' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the binary64 decimal exponent required by Lua"
for selection in \
  'gnu_compatibility_tuple=4.2.1' \
  'selected_builtin=__builtin_expect' \
  'selected_computed_goto=luaV_execute-jump-table' \
  'selected_attribute=noreturn' \
  'selected_attribute=visibility-internal' \
  'selected_operator=__extension__'; do
  grep -Fxq "$selection" "$work_directory/predicate-probe.txt" ||
    die "CCC does not select Lua's pinned compiler path: $selection"
done
grep -Fxq 'selected_double_mantissa=53' "$work_directory/hosted-probe.txt" ||
  die "the hosted float header does not expose Lua's required double mantissa width"
grep -Fxq 'selected_double_max_decimal_exponent=308' \
  "$work_directory/hosted-probe.txt" ||
  die "the hosted float header does not expose Lua's required double exponent"
grep -Fq '__builtin_huge_val' "$work_directory/hosted-probe.txt" ||
  die "the hosted math header does not select the pinned huge-value builtin"

{
  grep '^selected_' "$work_directory/predicate-probe.txt"
  printf '%s\n' \
    'selected_builtin=__builtin_huge_val' \
    'inline_assembly=none' \
    'wide_integer_use=none' \
    'variable_length_array_object=none' \
    'statement_expression=none'
} >"$work_directory/capability-inventory.txt"

: >"$CCC_LUA_COMMAND_LOG"
export CCC_LUA_SOURCE_ROOT="$source_directory/src"
export CCC_LUA_SOURCE_LOG="$work_directory/source-inputs.txt"
: >"$CCC_LUA_SOURCE_LOG"
expected_source_inputs="$work_directory/expected-source-inputs.txt"
find "$CCC_LUA_SOURCE_ROOT" -maxdepth 1 -type f -name '*.c' -print | \
  LC_ALL=C sort >"$expected_source_inputs"
expected_source_count=$(wc -l <"$expected_source_inputs" | tr -d '[:space:]')
[[ "$expected_source_count" == "$expected_translation_units" ]] ||
  die "Lua source archive contains $expected_source_count C inputs; expected $expected_translation_units"
make -C "$source_directory" -j"$jobs" linux \
  CC="$script_directory/ccc-cc" \
  2>&1 | tee "$work_directory/build.log"

[[ -x "$source_directory/src/lua" ]] || die "Lua interpreter was not produced"
[[ -x "$source_directory/src/luac" ]] || die "Lua bytecode compiler was not produced"
[[ -f "$source_directory/src/liblua.a" ]] || die "Lua static library was not produced"

actual_translation_units=$(grep -c '^ccc ' "$CCC_LUA_COMMAND_LOG" || true)
[[ "$actual_translation_units" == "$expected_translation_units" ]] ||
  die "Lua build translated $actual_translation_units C inputs; expected $expected_translation_units"
LC_ALL=C sort "$CCC_LUA_SOURCE_LOG" >"$CCC_LUA_SOURCE_LOG.sorted"
mv "$CCC_LUA_SOURCE_LOG.sorted" "$CCC_LUA_SOURCE_LOG"
cmp -s "$expected_source_inputs" "$CCC_LUA_SOURCE_LOG" ||
  die "Lua build did not translate the exact pinned set of C source files"
if grep '^link ' "$CCC_LUA_COMMAND_LOG" | grep -Eq '\.(c|i)( |$)'; then
  die "Lua native link command received a C source input"
fi
link_commands=$(grep -c '^link ' "$CCC_LUA_COMMAND_LOG" || true)
[[ "$link_commands" == 2 ]] ||
  die "Lua build invoked $link_commands native links; expected 2"
if grep '^link ' "$CCC_LUA_COMMAND_LOG" | grep -q -- ' -no-pie'; then
  die "Lua native links unexpectedly disabled PIE"
fi
linux_translations=$(grep '^ccc ' "$CCC_LUA_COMMAND_LOG" | \
  grep -c -- ' -DLUA_USE_LINUX' || true)
[[ "$linux_translations" == "$expected_translation_units" ]] ||
  die "Lua C translations did not all select the pinned Linux profile"
explicit_standard_translations=$(grep '^ccc ' "$CCC_LUA_COMMAND_LOG" | \
  grep -Ec -- ' -std=' || true)
[[ "$explicit_standard_translations" == 0 ]] ||
  die "Lua C translations unexpectedly overrode CCC's default GNU language mode"
if grep '^ccc ' "$CCC_LUA_COMMAND_LOG" | \
  grep -Eq -- '-DLUA_NOBUILTIN|-DLUA_USE_JUMPTABLE=0'; then
  die "Lua C translations disabled a compiler-selected source path"
fi

: >"$work_directory/elf-headers.txt"
: >"$work_directory/elf-dynamic-tags.txt"
for executable in lua luac; do
  binary="$source_directory/src/$executable"
  {
    printf '==> %s <==\n' "$executable"
    readelf --file-header "$binary"
  } >>"$work_directory/elf-headers.txt"
  {
    printf '==> %s <==\n' "$executable"
    readelf --dynamic "$binary"
  } >>"$work_directory/elf-dynamic-tags.txt"
  elf_type=$(readelf --file-header "$binary" | awk '/^[[:space:]]*Type:/{print $2; exit}')
  [[ "$elf_type" == DYN ]] ||
    die "Lua $executable is $elf_type rather than the required PIE executable type"
  if readelf --dynamic "$binary" | grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL'; then
    die "Lua $executable contains dynamic text relocations"
  fi
done

unset LUA_INIT LUA_INIT_5_5 LUA_PATH LUA_PATH_5_5 LUA_CPATH LUA_CPATH_5_5
"$source_directory/src/lua" -v \
  2>&1 | tee "$work_directory/version.log"
grep -Fq "Lua $version" "$work_directory/version.log" ||
  die "built Lua interpreter reports an unexpected version"

(
  cd "$test_directory"
  "$source_directory/src/lua" -e'_U=true' all.lua
) 2>&1 | tee "$work_directory/test.log"

grep -Fxq "$success_marker" "$work_directory/test.log" ||
  die "Lua test suite did not print its success marker"

printf 'Lua %s test artifacts: %s\n' "$version" "$work_directory"
