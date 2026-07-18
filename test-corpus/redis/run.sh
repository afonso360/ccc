#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_directory/../.." && pwd)
manifest="$script_directory/manifest.toml"
source "$script_directory/source-adjustment.sh"
source "$repository/test-corpus/adapter-environment.sh"

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
  resolved=$(command -v "$executable" 2>/dev/null) ||
    die "executable is not available: $executable"
  [[ -x "$resolved" ]] || die "file is not executable: $resolved"
  if [[ "$resolved" != /* ]]; then
    resolved="$(pwd -P)/$resolved"
  fi
  printf '%s\n' "$resolved"
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

verify_archive() {
  local path=$1
  local expected_bytes=$2
  local expected_sha256=$3
  local expected_sha3=$4
  local actual_bytes actual_sha256 actual_sha3

  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$(openssl dgst -sha256 "$path" | awk '{print $NF}')
  actual_sha3=$(openssl dgst -sha3-256 "$path" | awk '{print $NF}')
  [[ "$actual_bytes" == "$expected_bytes" ]] ||
    die "Redis archive size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] ||
    die "Redis archive SHA-256 mismatch"
  [[ "$actual_sha3" == "$expected_sha3" ]] ||
    die "Redis archive SHA3-256 mismatch"
}

verify_non_pie_executable() {
  local label=$1
  local program=$2
  local header_artifact=$3
  local dynamic_artifact=$4
  local program_type

  [[ -x "$program" ]] || die "$label executable was not produced: $program"
  readelf --file-header "$program" >"$header_artifact"
  readelf --dynamic "$program" >"$dynamic_artifact"
  program_type=$(awk '/^[[:space:]]*Type:/{print $2; exit}' "$header_artifact")
  [[ "$program_type" == EXEC ]] ||
    die "$label is $program_type rather than the pinned non-PIE executable type"
  if grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL' "$dynamic_artifact"; then
    die "$label contains dynamic text relocations"
  fi
}

require_adjusted_count() {
  local artifact=$1
  local label=$2
  local expected=$3
  local pattern=$4
  local file=$5
  local actual

  actual=$(grep -Fc -- "$pattern" "$file" || true)
  printf '%s=%s\n' "$label" "$actual" >>"$artifact"
  [[ "$actual" == "$expected" ]] ||
    die "Redis source adjustment has unexpected $label count: expected $expected, found $actual"
}

require_adjusted_regex_count() {
  local artifact=$1
  local label=$2
  local expected=$3
  local pattern=$4
  local file=$5
  local actual

  actual=$(grep -Ec -- "$pattern" "$file" || true)
  printf '%s=%s\n' "$label" "$actual" >>"$artifact"
  [[ "$actual" == "$expected" ]] ||
    die "Redis source adjustment has unexpected $label count: expected $expected, found $actual"
}

expanded_builtin_count() {
  local artifact=$1
  local builtin=$2
  local count

  count=$(awk -F= -v builtin="$builtin" '$1 == builtin { print $2; exit }' "$artifact")
  printf '%s\n' "${count:-0}"
}

require_expanded_builtin_count() {
  local artifact=$1
  local builtin=$2
  local expected=$3
  local actual

  [[ "$expected" =~ ^[0-9][0-9]*$ ]] ||
    die "Redis manifest has no valid expanded count for $builtin"
  actual=$(expanded_builtin_count "$artifact" "$builtin")
  [[ "$actual" == "$expected" ]] ||
    die "Redis expanded $builtin count mismatch: expected $expected, found $actual"
}

require_expanded_builtin_absent() {
  local artifact=$1
  local builtin=$2
  local actual

  actual=$(expanded_builtin_count "$artifact" "$builtin")
  [[ "$actual" == 0 ]] ||
    die "Redis unexpectedly expanded $builtin $actual times"
}

archive=${REDIS_SOURCE_ARCHIVE:-}
work_directory=${REDIS_WORK_DIR:-}
jobs=${REDIS_BUILD_JOBS:-2}

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
  echo "Redis build job count must be a positive integer: $jobs" >&2
  exit 2
}

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
  die "Redis execution validation requires x86-64 Linux"
[[ "$(id -u)" != 0 ]] ||
  die "Redis execution validation must run as a non-root user"

for tool in ar awk basename cmp cp dirname find gcc grep id make mkdir mktemp mv \
  openssl patch ranlib readelf rm sed sleep sort tar tee tr uname wc; do
  require_tool "$tool"
done
patch_identity=$(patch --version 2>&1) || die "unable to identify the patch implementation"
printf '%s\n' "$patch_identity" | grep -Fq 'GNU patch' ||
  die "Redis source adjustment requires GNU patch"

version=$(manifest_string version)
origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_bytes=$(manifest_integer archive_bytes)
expected_sha256=$(manifest_string archive_sha256)
expected_sha3=$(manifest_string archive_sha3_256)
expected_translation_units=$(manifest_integer source_translation_units)
expected_native_links=$(manifest_integer native_link_commands)
expected_adjustment_targets=$(manifest_integer source_adjustment_targets)
expected_classification_rewrites=$(manifest_integer source_adjustment_classification_calls_rewritten)
expected_xxhash_noop_definitions=$(manifest_integer source_adjustment_xxhash_ccc_noop_definitions)
expected_xxhash_guard_call_sites=$(manifest_integer source_adjustment_xxhash_guard_call_sites)
expected_xxhash_clang_guard_call_sites=$(manifest_integer source_adjustment_xxhash_clang_neon_guard_call_sites)
expected_xxhash_guard_expansions=$(manifest_integer source_adjustment_xxhash_selected_guard_expansions)
adjustment_name=$(manifest_string source_adjustment_patch)
adjustment_sha256=$(manifest_string source_adjustment_sha256)
adjustment_hash_name=$(manifest_string source_adjustment_hashes)
adjustment_hash_sha256=$(manifest_string source_adjustment_hashes_sha256)
adjustment_rationale=$(manifest_string source_adjustment_rationale)
portable_assert_name=$(manifest_string portable_assert_header)
portable_assert_sha256=$(manifest_string portable_assert_header_sha256)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-redis-$version.XXXXXX")
else
  if [[ -e "$work_directory" && ! -d "$work_directory" ]]; then
    echo "Redis work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "Redis work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")
printf '%s\n' "$patch_identity" >"$work_directory/patch-identity.txt"

if [[ -z "$archive" ]]; then
  cache_directory=${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}/ccc/corpus/redis
  mkdir -p "$cache_directory"
  archive="$cache_directory/$archive_name"
  download_archive "$origin" "$archive"
fi
archive=$(absolute_file "$archive")
verify_archive "$archive" "$expected_bytes" "$expected_sha256" "$expected_sha3"

source_parent="$work_directory/source"
mkdir -p "$source_parent"
tar -xzf "$archive" -C "$source_parent"
source_directory="$source_parent/redis-$version"
[[ -f "$source_directory/src/Makefile" ]] ||
  die "Redis source archive has an unexpected layout"
[[ -f "$source_directory/LICENSE.txt" ]] ||
  die "Redis source archive is missing LICENSE.txt"
grep -Fq "#define REDIS_VERSION \"$version\"" "$source_directory/src/version.h" ||
  die "Redis source version does not match the corpus pin"
grep -Fq 'GNU Affero General Public License' "$source_directory/LICENSE.txt" ||
  die "Redis source license does not contain the pinned AGPL option"
grep -Fq 'Redis Source Available License 2.0' "$source_directory/LICENSE.txt" ||
  die "Redis source license does not contain the pinned RSAL option"
grep -Fq 'Server Side Public License' "$source_directory/LICENSE.txt" ||
  die "Redis source license does not contain the pinned SSPL option"

apply_redis_source_adjustment \
  "$source_directory" \
  "$work_directory" \
  "$script_directory/$adjustment_name" \
  "$adjustment_sha256" \
  "$script_directory/$adjustment_hash_name" \
  "$adjustment_hash_sha256" \
  "$adjustment_rationale"

actual_adjustment_targets=$(grep -Evc '^[[:space:]]*(#|$)' \
  "$script_directory/$adjustment_hash_name")
[[ "$actual_adjustment_targets" == "$expected_adjustment_targets" ]] ||
  die "Redis source-adjustment inventory has $actual_adjustment_targets targets; expected $expected_adjustment_targets"

adjustment_audit="$work_directory/source-adjustment-audit.txt"
: >"$adjustment_audit"
require_adjusted_count "$adjustment_audit" upstream_statement_expression_cas_uses 0 \
  'atomicCompareExchange(size_t, zmalloc_peak' "$source_directory/src/zmalloc.c"
require_adjusted_count "$adjustment_audit" replacement_peak_cas_helper_references 2 \
  'zmalloc_compare_exchange_peak(' "$source_directory/src/zmalloc.c"
require_adjusted_count "$adjustment_audit" histogram_val_cas_uses 3 \
  '__sync_val_compare_and_swap' "$source_directory/deps/hdr_histogram/hdr_atomic.h"
require_adjusted_count "$adjustment_audit" histogram_exchange_uses 3 \
  '__sync_lock_test_and_set' "$source_directory/deps/hdr_histogram/hdr_atomic.h"
require_adjusted_count "$adjustment_audit" histogram_inline_assembly_uses 0 \
  'asm volatile' "$source_directory/deps/hdr_histogram/hdr_atomic.h"
require_adjusted_count "$adjustment_audit" hiredis_binary64_finite_checks 1 \
  'd >= -DBL_MAX && d <= DBL_MAX' "$source_directory/deps/hiredis/read.c"
require_adjusted_count "$adjustment_audit" hiredis_generic_finite_checks 0 \
  'isfinite(d)' "$source_directory/deps/hiredis/read.c"
require_adjusted_count "$adjustment_audit" lua_cjson_binary64_inf_calls 2 \
  'json_is_inf(num)' "$source_directory/deps/lua/src/lua_cjson.c"
require_adjusted_count "$adjustment_audit" lua_cjson_binary64_nan_calls 3 \
  'json_is_nan(num)' "$source_directory/deps/lua/src/lua_cjson.c"
require_adjusted_count "$adjustment_audit" lua_cjson_generic_inf_calls 0 \
  'isinf(num)' "$source_directory/deps/lua/src/lua_cjson.c"
require_adjusted_count "$adjustment_audit" lua_cjson_generic_nan_calls 0 \
  'isnan(num)' "$source_directory/deps/lua/src/lua_cjson.c"
require_adjusted_count "$adjustment_audit" lua_cmsgpack_binary64_inf_calls 1 \
  'cmsgpack_is_inf(x)' "$source_directory/deps/lua/src/lua_cmsgpack.c"
require_adjusted_count "$adjustment_audit" lua_cmsgpack_generic_inf_calls 0 \
  'isinf(x)' "$source_directory/deps/lua/src/lua_cmsgpack.c"
require_adjusted_count "$adjustment_audit" xxhash_ccc_noop_definitions \
  "$expected_xxhash_noop_definitions" \
  '#  define XXH_COMPILER_GUARD(var) ((void)sizeof("ccc-xxhash-compiler-guard-noop"))' \
  "$source_directory/deps/xxhash/xxhash.h"
require_adjusted_regex_count "$adjustment_audit" xxhash_ordinary_guard_call_sites \
  "$expected_xxhash_guard_call_sites" \
  '^[[:space:]]+XXH_COMPILER_GUARD\(' \
  "$source_directory/deps/xxhash/xxhash.h"
require_adjusted_regex_count "$adjustment_audit" xxhash_clang_neon_guard_call_sites \
  "$expected_xxhash_clang_guard_call_sites" \
  '^[[:space:]]+XXH_COMPILER_GUARD_CLANG_NEON\(' \
  "$source_directory/deps/xxhash/xxhash.h"
require_adjusted_count "$adjustment_audit" xxhash_gnu_guard_definitions 1 \
  '#  define XXH_COMPILER_GUARD(var) __asm__("" : "+r" (var))' \
  "$source_directory/deps/xxhash/xxhash.h"
require_adjusted_count "$adjustment_audit" xxhash_upstream_noop_guard_definitions 1 \
  '#  define XXH_COMPILER_GUARD(var) ((void)0)' \
  "$source_directory/deps/xxhash/xxhash.h"
actual_classification_rewrites=$((1 + 2 + 3 + 1))
[[ "$actual_classification_rewrites" == "$expected_classification_rewrites" ]] ||
  die "Redis source-adjustment classification count does not match the manifest"
printf 'binary64_classification_calls_rewritten=%s\n' \
  "$actual_classification_rewrites" >>"$adjustment_audit"

portable_assert_path="$script_directory/$portable_assert_name"
[[ "$(redis_sha256_file "$portable_assert_path")" == "$portable_assert_sha256" ]] ||
  die "Redis portable-assert header SHA-256 mismatch"
portable_assert_include=$(absolute_directory "$(dirname "$portable_assert_path")")
cp "$portable_assert_path" "$work_directory/portable-assert.h"

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_LINK_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
archiver=$(resolve_executable ar)
archive_indexer=$(resolve_executable ranlib)
export CCC CCC_RESOURCE_DIR CCC_LINK_CC
export CCC_REDIS_COMMAND_LOG="$work_directory/compile-commands.txt"

record_native_gcc_driver \
  Redis "$CCC_LINK_CC" \
  "$work_directory/link-driver-identity.txt" \
  "$work_directory/link-driver-macros.txt"

# Make environment injection and package-manager build flags can change both
# Redis's selected source paths and its ABI. The corpus owns the entire profile.
clear_ambient_make_injection
unset ARFLAGS BUILD_TLS C11_ATOMIC CFLAGS CPPFLAGS DEBUG DEBUG_FLAGS
unset ENABLE_LTO HIREDIS_CFLAGS HIREDIS_LDFLAGS LDFLAGS LIBS MALLOC
unset OPT OPTIMIZATION REDIS_CFLAGS REDIS_LDFLAGS SANITIZER SKIP_VEC_SETS
unset USE_JEMALLOC USE_SYSTEMD USE_TCMALLOC USE_TCMALLOC_MINIMAL
unset REDISCLI_AUTH REDISCLI_HISTFILE
unset CCC_REDIS_PREPROCESS_DIR CCC_REDIS_SOURCE_LOG CCC_REDIS_SOURCE_ROOT
export LC_ALL=C TZ=UTC SOURCE_DATE_EPOCH=1779667200

: >"$CCC_REDIS_COMMAND_LOG"
"$script_directory/ccc-cc" -std=gnu11 -I"$portable_assert_include" -dM -E \
  "$script_directory/predicate-probe.c" >"$work_directory/effective-macros.txt"
"$script_directory/ccc-cc" -std=gnu11 -I"$portable_assert_include" -P -E \
  "$script_directory/predicate-probe.c" >"$work_directory/predicate-probe.txt"

grep -Fxq '#define __GNUC__ 4' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC__ value"
grep -Fxq '#define __GNUC_MINOR__ 2' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_MINOR__ value"
grep -Fxq '#define __GNUC_PATCHLEVEL__ 1' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_PATCHLEVEL__ value"
grep -Fxq '#define __STDC_NO_ATOMICS__ 1' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned C11 atomic denial"
grep -Fxq '#define CCC_REDIS_PORTABLE_ASSERT 1' "$work_directory/effective-macros.txt" ||
  die "CCC does not select Redis's portable hosted assert header"
grep -Fxq '#define CCC_REDIS_HOSTED_FEATURES_PRIMED 1' "$work_directory/effective-macros.txt" ||
  die "Redis portable hosted assert header did not prime glibc features"
grep -Fxq '#define __ASSERT_FUNCTION __func__' "$work_directory/effective-macros.txt" ||
  die "Redis portable hosted assert header does not select __func__"
if grep -Fq '#define __STRICT_ANSI__' "$work_directory/effective-macros.txt"; then
  die "Redis portable hosted assert header did not restore GNU mode"
fi
for selection in \
  'gnu_compatibility_tuple=4.2.1' \
  'selected_assert=standard-c-macro-gnu-mode-restored' \
  'selected_c11_atomic_surface=unavailable' \
  'selected_core_atomic_surface=sync-builtin' \
  'selected_upstream_hdr_atomic_surface=x86-inline-assembly'; do
  grep -Fxq "$selection" "$work_directory/predicate-probe.txt" ||
    die "CCC does not select Redis's pinned compiler path: $selection"
done

{
  grep -E '^(gnu_compatibility_tuple|selected_|available_builtin_)' \
    "$work_directory/predicate-probe.txt"
  printf '%s\n' \
    'allocator=libc' \
    'assertions=enabled-with-portable-hosted-header' \
    'c11-atomics=unavailable' \
    'dynamic-stack-storage=none' \
    'inline-assembly=none-after-source-adjustment' \
    'lto=disabled' \
    'module-bundle=excluded' \
    'systemd=disabled' \
    'tls=disabled' \
    'vector-sets=disabled'
} >"$work_directory/capability-inventory.txt"

export CCC_REDIS_SOURCE_ROOT="$source_directory"
export CCC_REDIS_SOURCE_LOG="$work_directory/source-inputs.txt"
export CCC_REDIS_PREPROCESS_DIR="$work_directory/effective-preprocessed"
mkdir -p "$CCC_REDIS_PREPROCESS_DIR"
: >"$CCC_REDIS_COMMAND_LOG"
: >"$CCC_REDIS_SOURCE_LOG"

source_inventory="$script_directory/source-set.txt"
inventory_count=$(wc -l <"$source_inventory" | tr -d '[:space:]')
inventory_unique_count=$(LC_ALL=C sort -u "$source_inventory" | wc -l | tr -d '[:space:]')
[[ "$inventory_count" == "$expected_translation_units" ]] ||
  die "Redis source inventory has $inventory_count entries; expected $expected_translation_units"
[[ "$inventory_unique_count" == "$expected_translation_units" ]] ||
  die "Redis source inventory contains duplicate entries"

expected_source_inputs="$work_directory/expected-source-inputs.txt"
: >"$expected_source_inputs"
while IFS= read -r relative_source; do
  [[ -n "$relative_source" && "$relative_source" != /* && "$relative_source" != *..* ]] ||
    die "invalid Redis source inventory entry: $relative_source"
  [[ -f "$source_directory/$relative_source" ]] ||
    die "Redis source inventory entry is absent from the archive: $relative_source"
  printf '%s/%s\n' "$source_directory" "$relative_source" >>"$expected_source_inputs"
done <"$source_inventory"
LC_ALL=C sort "$expected_source_inputs" >"$expected_source_inputs.sorted"
mv "$expected_source_inputs.sorted" "$expected_source_inputs"

compiler_adapter="$script_directory/ccc-cc"
portable_assert_flag="-I$portable_assert_include"
redis_std="-std=gnu11 -DREDIS_STATIC=''"
redis_warn='-Wall -W -Wno-missing-field-initializers -Werror=deprecated-declarations -Wstrict-prototypes'

build_redis_settings=(
  CC="$compiler_adapter"
  DEPENDENCY_TARGETS=
  STD="$redis_std"
  WARN="$redis_warn"
  OPT=-O2
  OPTIMIZATION=-O2
  ENABLE_LTO=
  DEBUG=
  CFLAGS="$portable_assert_flag"
  LDFLAGS=-no-pie
  REDIS_CFLAGS=
  REDIS_LDFLAGS=
  MALLOC=libc
  BUILD_TLS=no
  USE_SYSTEMD=no
  SKIP_VEC_SETS=yes
  C11_ATOMIC=no
)

{
  # Persist the exact core settings without invoking Redis's broad dependency
  # umbrella, which also builds executables and shared libraries outside this
  # corpus. Each required static component is built explicitly below.
  make -C "$source_directory/src" persist-settings "${build_redis_settings[@]}"

  make -C "$source_directory/deps/hiredis" -j"$jobs" static \
    CC="$compiler_adapter" AR="$archiver" \
    OPTIMIZATION=-O2 DEBUG_FLAGS= CFLAGS= CPPFLAGS= LDFLAGS= \
    HIREDIS_CFLAGS="$portable_assert_flag" HIREDIS_LDFLAGS=

  make -C "$source_directory/deps/linenoise" -j"$jobs" linenoise.o \
    CC="$compiler_adapter" STD= OPT=-O2 DEBUG= CFLAGS="$portable_assert_flag" LDFLAGS=

  make -C "$source_directory/deps/lua/src" -j"$jobs" a \
    CC="$compiler_adapter" \
    CFLAGS="-O2 -Wall $portable_assert_flag -DLUA_ANSI -DENABLE_CJSON_GLOBAL -DREDIS_STATIC='' -DLUA_USE_MKSTEMP" \
    AR="$archiver rcs" RANLIB="$archive_indexer" MYLDFLAGS= MYLIBS=

  make -C "$source_directory/deps/hdr_histogram" -j"$jobs" libhdrhistogram.a \
    CC="$compiler_adapter" AR="$archiver" ARFLAGS=rcs \
    STD=-std=gnu11 OPT=-O2 DEBUG= CFLAGS="$portable_assert_flag" LDFLAGS=

  make -C "$source_directory/deps/fpconv" -j"$jobs" libfpconv.a \
    CC="$compiler_adapter" AR="$archiver" ARFLAGS=rcs \
    STD=-std=gnu11 OPT=-O2 DEBUG= CFLAGS="$portable_assert_flag" LDFLAGS=

  make -C "$source_directory/deps/xxhash" -j"$jobs" libxxhash.a \
    CC="$compiler_adapter" AR="$archiver" \
    CFLAGS="-O2 $portable_assert_flag" CPPFLAGS= LDFLAGS= DEBUGFLAGS= MOREFLAGS= \
    DISPATCH=0 LIBXXH_DISPATCH=0

  make -C "$source_directory/deps/tre" -j"$jobs" libtre.a \
    CC="$compiler_adapter" AR="$archiver" ARFLAGS=rcs \
    STD=-std=gnu11 OPT=-O2 DEBUG= CFLAGS="$portable_assert_flag" LDFLAGS=

  # Avoid Redis's developer-only multi-source dependency scan. Per-object MMD
  # inputs remain on every audited CCC translation command.
  : >"$source_directory/src/Makefile.dep"
  make -C "$source_directory/src" -j"$jobs" redis-server redis-cli \
    "${build_redis_settings[@]}"
} 2>&1 | tee "$work_directory/build.log"

server="$source_directory/src/redis-server"
client="$source_directory/src/redis-cli"
[[ -x "$server" ]] || die "Redis server was not produced"
[[ -x "$client" ]] || die "Redis CLI was not produced"
[[ -f "$source_directory/deps/hiredis/libhiredis.a" ]] ||
  die "hiredis static library was not produced"
[[ -f "$source_directory/deps/lua/src/liblua.a" ]] ||
  die "Lua static library was not produced"
[[ -f "$source_directory/deps/hdr_histogram/libhdrhistogram.a" ]] ||
  die "HDR Histogram static library was not produced"
[[ -f "$source_directory/deps/fpconv/libfpconv.a" ]] ||
  die "fpconv static library was not produced"
[[ -f "$source_directory/deps/xxhash/libxxhash.a" ]] ||
  die "xxHash static library was not produced"
[[ -f "$source_directory/deps/tre/libtre.a" ]] ||
  die "TRE static library was not produced"

actual_translation_units=$(grep -c '^ccc ' "$CCC_REDIS_COMMAND_LOG" || true)
[[ "$actual_translation_units" == "$expected_translation_units" ]] ||
  die "Redis build translated $actual_translation_units C inputs; expected $expected_translation_units"
actual_unique_sources=$(LC_ALL=C sort -u "$CCC_REDIS_SOURCE_LOG" | wc -l | tr -d '[:space:]')
[[ "$actual_unique_sources" == "$expected_translation_units" ]] ||
  die "Redis build did not translate each pinned C input exactly once"
LC_ALL=C sort "$CCC_REDIS_SOURCE_LOG" >"$CCC_REDIS_SOURCE_LOG.sorted"
mv "$CCC_REDIS_SOURCE_LOG.sorted" "$CCC_REDIS_SOURCE_LOG"
cmp -s "$expected_source_inputs" "$CCC_REDIS_SOURCE_LOG" ||
  die "Redis build did not translate the exact pinned C source inventory"

actual_preprocess_commands=$(grep -c '^preprocess ' "$CCC_REDIS_COMMAND_LOG" || true)
[[ "$actual_preprocess_commands" == "$expected_translation_units" ]] ||
  die "Redis build captured $actual_preprocess_commands preprocessing passes; expected $expected_translation_units"
actual_preprocessed_inputs=$(find "$CCC_REDIS_PREPROCESS_DIR" -type f -name '*.i' |
  wc -l | tr -d '[:space:]')
[[ "$actual_preprocessed_inputs" == "$expected_translation_units" ]] ||
  die "Redis build retained $actual_preprocessed_inputs preprocessed inputs; expected $expected_translation_units"
actual_nonempty_preprocessed_inputs=$(find "$CCC_REDIS_PREPROCESS_DIR" -type f -name '*.i' ! -empty |
  wc -l | tr -d '[:space:]')
[[ "$actual_nonempty_preprocessed_inputs" == "$expected_translation_units" ]] ||
  die "Redis preprocessing capture produced an empty input"

preprocessed_source_inputs="$work_directory/preprocessed-source-inputs.txt"
while IFS= read -r preprocessed_input; do
  relative_input=${preprocessed_input#"$CCC_REDIS_PREPROCESS_DIR/"}
  printf '%s\n' "${relative_input%.i}"
done < <(find "$CCC_REDIS_PREPROCESS_DIR" -type f -name '*.i') |
  LC_ALL=C sort >"$preprocessed_source_inputs"
LC_ALL=C sort "$source_inventory" >"$work_directory/expected-preprocessed-source-inputs.txt"
cmp -s "$work_directory/expected-preprocessed-source-inputs.txt" "$preprocessed_source_inputs" ||
  die "Redis preprocessing capture did not cover the exact pinned C source inventory"
rm -f -- "$work_directory/expected-preprocessed-source-inputs.txt"

expanded_builtin_occurrences="$work_directory/expanded-builtin-occurrences.txt"
expanded_builtins="$work_directory/expanded-builtins.txt"
inline_assembly_occurrences="$work_directory/inline-assembly-occurrences.txt"
inline_assembly_inventory="$work_directory/inline-assembly-inventory.txt"
xxhash_guard_markers="$work_directory/xxhash-guard-markers.txt"
xxhash_guard_identifiers="$work_directory/xxhash-guard-identifiers.txt"
: >"$expanded_builtin_occurrences"
: >"$inline_assembly_occurrences"
: >"$xxhash_guard_markers"
: >"$xxhash_guard_identifiers"
while IFS= read -r -d '' preprocessed_input; do
  grep -Eo '__builtin_[[:alnum:]_]+|__sync_[[:alnum:]_]+' "$preprocessed_input" \
    >>"$expanded_builtin_occurrences" || true
  grep -HEn '(^|[^[:alnum:]_])(__asm__|__asm|asm)[[:space:]]*(volatile[[:space:]]*)?\(' \
    "$preprocessed_input" >>"$inline_assembly_occurrences" || true
  grep -Fo 'ccc-xxhash-compiler-guard-noop' "$preprocessed_input" \
    >>"$xxhash_guard_markers" || true
  grep -Eo 'XXH_COMPILER_GUARD(_CLANG_NEON)?' "$preprocessed_input" \
    >>"$xxhash_guard_identifiers" || true
done < <(find "$CCC_REDIS_PREPROCESS_DIR" -type f -name '*.i' -print0)
LC_ALL=C sort "$expanded_builtin_occurrences" |
  awk '
    NR == 1 { name = $0; count = 1; next }
    $0 == name { count++; next }
    { print name "=" count; name = $0; count = 1 }
    END { if (NR != 0) print name "=" count }
  ' >"$expanded_builtins"
rm -f -- "$expanded_builtin_occurrences"
if [[ -s "$inline_assembly_occurrences" ]]; then
  mv "$inline_assembly_occurrences" "$inline_assembly_inventory"
  die "Redis preprocessing capture selected inline assembly"
fi
actual_xxhash_guard_expansions=$(wc -l <"$xxhash_guard_markers" | tr -d '[:space:]')
[[ "$actual_xxhash_guard_expansions" == "$expected_xxhash_guard_expansions" ]] ||
  die "Redis expanded the xxHash CCC guard $actual_xxhash_guard_expansions times; expected $expected_xxhash_guard_expansions"
[[ ! -s "$xxhash_guard_identifiers" ]] ||
  die "Redis preprocessing left an unexpanded xxHash compiler guard"
{
  printf '%s\n' 'expanded_inline_assembly_forms=0'
  printf 'xxhash_ccc_noop_guard_expansions=%s\n' "$actual_xxhash_guard_expansions"
  printf '%s\n' 'xxhash_unexpanded_guard_identifiers=0'
} >"$inline_assembly_inventory"
rm -f -- "$inline_assembly_occurrences"
rm -f -- "$xxhash_guard_markers" "$xxhash_guard_identifiers"

for builtin in \
  __builtin_bswap64 \
  __builtin_clz \
  __builtin_clzl \
  __builtin_clzll \
  __builtin_ctzll \
  __builtin_expect \
  __builtin_popcount \
  __builtin_popcountll \
  __builtin_prefetch \
  __sync_add_and_fetch \
  __sync_bool_compare_and_swap \
  __sync_fetch_and_add \
  __sync_lock_test_and_set \
  __sync_sub_and_fetch \
  __sync_synchronize \
  __sync_val_compare_and_swap; do
  [[ "$(expanded_builtin_count "$expanded_builtins" "$builtin")" != 0 ]] ||
    die "Redis selected builtin was not present after preprocessing: $builtin"
done

require_expanded_builtin_count "$expanded_builtins" __builtin_bswap64 \
  "$(manifest_integer expanded_builtin_bswap64)"
require_expanded_builtin_count "$expanded_builtins" __builtin_clz \
  "$(manifest_integer expanded_builtin_clz)"
require_expanded_builtin_count "$expanded_builtins" __builtin_clzl \
  "$(manifest_integer expanded_builtin_clzl)"
require_expanded_builtin_count "$expanded_builtins" __builtin_clzll \
  "$(manifest_integer expanded_builtin_clzll)"
require_expanded_builtin_count "$expanded_builtins" __builtin_ctzll \
  "$(manifest_integer expanded_builtin_ctzll)"
require_expanded_builtin_count "$expanded_builtins" __builtin_popcount \
  "$(manifest_integer expanded_builtin_popcount)"
require_expanded_builtin_count "$expanded_builtins" __builtin_popcountll \
  "$(manifest_integer expanded_builtin_popcountll)"
require_expanded_builtin_count "$expanded_builtins" __builtin_prefetch \
  "$(manifest_integer expanded_builtin_prefetch)"

for builtin in \
  __builtin_add_overflow \
  __builtin_assume_aligned \
  __builtin_bswap32 \
  __builtin_cpu_supports \
  __builtin_ctz \
  __builtin_ctzl \
  __builtin_frame_address \
  __builtin_mul_overflow \
  __builtin_popcountl \
  __builtin_return_address \
  __builtin_sub_overflow \
  __builtin_unreachable; do
  require_expanded_builtin_absent "$expanded_builtins" "$builtin"
done
if grep -Eq '^__atomic_' "$expanded_builtins"; then
  die "Redis unexpectedly selected the __atomic builtin family"
fi

# The expanded sources are deliberately temporary: the compact source list
# and exact builtin counts retain the proof without retaining a second source
# distribution in the validation artifacts.
rm -rf -- "$CCC_REDIS_PREPROCESS_DIR"

link_commands=$(grep -c '^link ' "$CCC_REDIS_COMMAND_LOG" || true)
[[ "$link_commands" == "$expected_native_links" ]] ||
  die "Redis build invoked $link_commands native links; expected $expected_native_links"
if grep '^link ' "$CCC_REDIS_COMMAND_LOG" | grep -Eq '\.(c|i)( |$)'; then
  die "Redis native link command received a C source input"
fi
non_pie_links=$(grep '^link ' "$CCC_REDIS_COMMAND_LOG" | grep -c -- ' -no-pie' || true)
[[ "$non_pie_links" == "$expected_native_links" ]] ||
  die "Redis native links did not all receive the pinned -no-pie option"

gnu11_translations=$(grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -c -- ' -std=gnu11' || true)
[[ "$gnu11_translations" == "$expected_translation_units" ]] ||
  die "Redis C translations did not all use the pinned GNU language mode"
if grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" | grep -Fq -- ' -DNDEBUG'; then
  die "Redis C translations unexpectedly disabled assertions"
fi
portable_assert_translations=$(grep '^ccc ' "$CCC_REDIS_COMMAND_LOG" |
  grep -Fc -- " -I$portable_assert_include" || true)
[[ "$portable_assert_translations" == "$expected_translation_units" ]] ||
  die "Redis C translations did not all select the portable hosted assert header"
if grep -Eq -- '-flto|-DUSE_JEMALLOC|-DUSE_OPENSSL|-DINCLUDE_VEC_SETS|-DHAVE_LIBSYSTEMD' \
  "$CCC_REDIS_COMMAND_LOG"; then
  die "Redis build selected a capability outside the pinned profile"
fi

for builtin in \
  __builtin_bswap64 \
  __builtin_clz \
  __builtin_clzl \
  __builtin_clzll \
  __builtin_ctzll \
  __builtin_expect \
  __builtin_popcount \
  __builtin_popcountll \
  __builtin_prefetch \
  __sync_add_and_fetch \
  __sync_bool_compare_and_swap \
  __sync_fetch_and_add \
  __sync_lock_test_and_set \
  __sync_sub_and_fetch \
  __sync_synchronize \
  __sync_val_compare_and_swap; do
  grep -Fxq "available_builtin_1=$builtin" "$work_directory/predicate-probe.txt" ||
    die "CCC does not provide Redis's selected builtin: $builtin"
done

verify_non_pie_executable \
  redis-server "$server" \
  "$work_directory/redis-server-elf-header.txt" \
  "$work_directory/redis-server-elf-dynamic.txt"
verify_non_pie_executable \
  redis-cli "$client" \
  "$work_directory/redis-cli-elf-header.txt" \
  "$work_directory/redis-cli-elf-dynamic.txt"
{
  printf '%s\n' '==> redis-server <=='
  sed -n '1,$p' "$work_directory/redis-server-elf-header.txt"
  printf '%s\n' '==> redis-cli <=='
  sed -n '1,$p' "$work_directory/redis-cli-elf-header.txt"
} >"$work_directory/elf-headers.txt"
{
  printf '%s\n' '==> redis-server <=='
  sed -n '1,$p' "$work_directory/redis-server-elf-dynamic.txt"
  printf '%s\n' '==> redis-cli <=='
  sed -n '1,$p' "$work_directory/redis-cli-elf-dynamic.txt"
} >"$work_directory/elf-dynamic-tags.txt"

{
  "$server" --version
  "$client" --version
} 2>&1 | tee "$work_directory/version.log"
grep -Fq "v=$version" "$work_directory/version.log" ||
  die "built Redis server reports an unexpected version"
grep -Fq "redis-cli $version" "$work_directory/version.log" ||
  die "built Redis CLI reports an unexpected version"

socket="$work_directory/redis.sock"
(( ${#socket} < 100 )) ||
  die "Redis work directory is too long for a portable Unix-domain socket path"
server_data="$work_directory/server-data"
mkdir -p "$server_data"
server_pid=
cleanup_server() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
}
trap cleanup_server EXIT INT TERM

"$server" \
  --port 0 \
  --unixsocket "$socket" \
  --unixsocketperm 700 \
  --dir "$server_data" \
  --save '' \
  --appendonly no \
  --daemonize no \
  >"$work_directory/server.log" 2>&1 &
server_pid=$!

ready=false
for ((attempt = 0; attempt < 200; attempt++)); do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid" || true
    server_pid=
    die "Redis server exited before accepting smoke-test commands"
  fi
  if [[ -S "$socket" ]] &&
    [[ "$("$client" -s "$socket" --raw PING 2>/dev/null || true)" == PONG ]]; then
    ready=true
    break
  fi
  sleep 0.05
done
$ready || die "Redis server did not become ready for the focused smoke test"

redis_command() {
  "$client" -s "$socket" --raw "$@"
}

{
  response=$(redis_command PING)
  printf 'PING => %s\n' "$response"
  [[ "$response" == PONG ]] || die "Redis PING smoke check failed"

  response=$(redis_command SET corpus:key ccc)
  printf 'SET corpus:key ccc => %s\n' "$response"
  [[ "$response" == OK ]] || die "Redis SET smoke check failed"
  response=$(redis_command GET corpus:key)
  printf 'GET corpus:key => %s\n' "$response"
  [[ "$response" == ccc ]] || die "Redis GET smoke check failed"

  response=$(redis_command INCR corpus:counter)
  printf 'INCR corpus:counter => %s\n' "$response"
  [[ "$response" == 1 ]] || die "first Redis INCR smoke check failed"
  response=$(redis_command INCR corpus:counter)
  printf 'INCR corpus:counter => %s\n' "$response"
  [[ "$response" == 2 ]] || die "second Redis INCR smoke check failed"

  response=$(redis_command LPUSH corpus:list alpha beta)
  printf 'LPUSH corpus:list alpha beta => %s\n' "$response"
  [[ "$response" == 2 ]] || die "Redis LPUSH smoke check failed"
  response=$(redis_command LRANGE corpus:list 0 -1)
  printf 'LRANGE corpus:list 0 -1 => %s\n' "$response"
  [[ "$response" == $'beta\nalpha' ]] || die "Redis LRANGE smoke check failed"

  response=$(redis_command HSET corpus:hash field value)
  printf 'HSET corpus:hash field value => %s\n' "$response"
  [[ "$response" == 1 ]] || die "Redis HSET smoke check failed"
  response=$(redis_command HGET corpus:hash field)
  printf 'HGET corpus:hash field => %s\n' "$response"
  [[ "$response" == value ]] || die "Redis HGET smoke check failed"

  response=$(redis_command EVAL "return redis.call('GET', KEYS[1])" 1 corpus:key)
  printf 'EVAL GET corpus:key => %s\n' "$response"
  [[ "$response" == ccc ]] || die "Redis EVAL smoke check failed"
  response=$(redis_command DBSIZE)
  printf 'DBSIZE => %s\n' "$response"
  [[ "$response" == 4 ]] || die "Redis DBSIZE smoke check failed"

  redis_command SHUTDOWN NOSAVE
  printf '%s\n' 'SHUTDOWN NOSAVE => accepted'
} >"$work_directory/smoke.log" 2>&1

for ((attempt = 0; attempt < 200; attempt++)); do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid"
    server_pid=
    break
  fi
  sleep 0.05
done
[[ -z "$server_pid" ]] || die "Redis server did not exit after SHUTDOWN NOSAVE"
trap - EXIT INT TERM

printf 'Redis %s validation artifacts: %s\n' "$version" "$work_directory"
