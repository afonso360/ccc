#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_directory/../.." && pwd)
manifest="$script_directory/manifest.toml"
source "$script_directory/suite-plan.sh"
source "$script_directory/source-adjustment.sh"
source "$repository/test-corpus/adapter-environment.sh"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$manifest"
}

manifest_integer() {
  sed -n "s/^$1 = \([0-9][0-9]*\)$/\1/p" "$manifest"
}

usage() {
  echo "usage: $0 [--archive PATH] [--work-dir PATH] [--jobs COUNT] [--suite veryquick|quick|all|full]" >&2
}

die() {
  echo "$*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "required tool is not available: $1"
}

verify_pie_executable() {
  local program=$1
  local header_artifact=$2
  local dynamic_artifact=$3
  local program_type

  [[ -x "$program" ]] || die "SQLite executable was not produced: $program"
  readelf --file-header "$program" >"$header_artifact"
  readelf --dynamic "$program" >"$dynamic_artifact"
  program_type=$(awk '/^[[:space:]]*Type:/{print $2; exit}' "$header_artifact")
  [[ "$program_type" == DYN ]] ||
    die "SQLite $program is $program_type rather than the required PIE executable type"
  if grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL' "$dynamic_artifact"; then
    die "SQLite $program contains dynamic text relocations"
  fi
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
suite=veryquick

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
    --suite)
      (($# >= 2)) || {
        usage
        exit 2
      }
      suite=$2
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

select_sqlite_suite "$suite" || {
  usage
  exit 2
}

[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || {
  echo "SQLite build job count must be a positive integer: $jobs" >&2
  exit 2
}

for tool in awk basename cat cmp cp dirname find grep id make mkdir mktemp mv openssl patch readelf rm sed sort tee tr unzip wc; do
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
expected_strict_ansi_predicate_uses=$(manifest_integer generated_strict_ansi_predicate_uses)
expected_strict_ansi_negated_uses=$(manifest_integer generated_strict_ansi_negated_uses)
adjustment_patch=$(manifest_string test_adjustment_patch)
expected_adjustment_sha256=$(manifest_string test_adjustment_sha256)
adjustment_target=$(manifest_string test_adjustment_target)
expected_adjustment_target_before_sha256=$(manifest_string test_adjustment_target_before_sha256)
expected_adjustment_target_after_sha256=$(manifest_string test_adjustment_target_after_sha256)
adjustment_rationale=$(manifest_string test_adjustment_rationale)
expected_testfixture_translation_units=$(manifest_integer testfixture_translation_units)
expected_fuzzcheck_translation_units=$(manifest_integer fuzzcheck_translation_units)
expected_fuzzcheck_support_translation_units=$(manifest_integer fuzzcheck_support_translation_units)

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

build_directory="$work_directory/build"
mkdir -p "$build_directory"
: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_LINK_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
export CCC CCC_RESOURCE_DIR CCC_LINK_CC
export CCC_SQLITE_COMMAND_LOG="$work_directory/build/configure-commands.txt"

record_native_gcc_driver \
  SQLite "$CCC_LINK_CC" \
  "$build_directory/link-driver-identity.txt" \
  "$build_directory/link-driver-macros.txt"

# Ambient build flags or configure cache hooks would change the selected C
# surface without changing the checked-in corpus contract.
clear_ambient_make_injection
unset BCC BUILD_CC CFLAGS CONFIG_SITE CPPFLAGS LDFLAGS LIBS
unset OPTS TCC TESTOPTS

cd "$build_directory"
: >"$CCC_SQLITE_COMMAND_LOG"

# glibc exposes isnan as a type-generic macro whose expansion mentions
# __isnanl even for a double operand. CCC deliberately has no long-double ABI,
# so the configure link probe is not a sufficient test of whether that macro is
# usable. Selecting SQLite's own binary64 test keeps the configured surface
# aligned with the compiler's advertised ABI without changing production C.
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
if [[ "$suite" == all || "$suite" == full ]]; then
  for required in \
    available_hosted_builtin=__builtin_inff \
    available_hosted_builtin=__builtin_nanf; do
    grep -Fxq "$required" predicate-probe.txt ||
      die "CCC does not expose SQLite's required hosted builtin: ${required#*=}"
  done
fi
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
actual_strict_ansi_predicate_uses=$(grep -Fc 'defined(__STRICT_ANSI__)' sqlite3.c)
actual_strict_ansi_negated_uses=$(grep -Fc '!defined(__STRICT_ANSI__)' sqlite3.c)
[[ "$actual_strict_ansi_predicate_uses" == "$expected_strict_ansi_predicate_uses" ]] ||
  die "generated sqlite3.c changed its __STRICT_ANSI__ predicate surface"
[[ "$actual_strict_ansi_negated_uses" == "$expected_strict_ansi_negated_uses" ]] ||
  die "generated sqlite3.c changed its negated __STRICT_ANSI__ predicate surface"

apply_sqlite_test_adjustment \
  "$source_directory" \
  "$build_directory" \
  "$script_directory/adjustments/$adjustment_patch" \
  "$expected_adjustment_sha256" \
  "$adjustment_target" \
  "$expected_adjustment_target_before_sha256" \
  "$expected_adjustment_target_after_sha256" \
  "$adjustment_rationale"

raw_expected_source_inputs="$build_directory/expected-testfixture-source-inputs.raw"
expected_source_inputs="$build_directory/expected-testfixture-source-inputs.txt"
make --silent --no-print-directory -f Makefile \
  -f "$script_directory/source-set.mk" \
  ccc-print-testfixture-sources >"$raw_expected_source_inputs"
: >"$expected_source_inputs"
while IFS= read -r source; do
  absolute_file "$source"
done <"$raw_expected_source_inputs" >>"$expected_source_inputs"
LC_ALL=C sort "$expected_source_inputs" >"$expected_source_inputs.sorted"
mv "$expected_source_inputs.sorted" "$expected_source_inputs"
rm -f -- "$raw_expected_source_inputs"
expected_source_count=$(wc -l <"$expected_source_inputs" | tr -d '[:space:]')
[[ "$expected_source_count" == "$expected_testfixture_translation_units" ]] ||
  die "SQLite testfixture declares $expected_source_count C inputs; expected $expected_testfixture_translation_units"

{
  grep -E '^(selected_builtin|available_hosted_builtin)=' predicate-probe.txt
  printf '%s\n' \
    'inline_assembly=none' \
    'wide_integer_use=none' \
    'variable_length_array_object=none' \
    'statement_expression=none' \
    'computed_goto=none'
  if [[ "$suite" == full ]]; then
    printf '%s\n' \
      'fuzzcheck_feature=SQLITE_ENABLE_STMT_SCANSTATUS' \
      'fuzzcheck_amalgamation_language_mode=gnu11' \
      'fuzzcheck_support_language_mode=gnu11' \
      'fuzzcheck_hwtime_predicate_override=__STRICT_ANSI__=1' \
      'fuzzcheck_additional_predicate_effect=SQLITE_INLINE-disabled' \
      'fuzzcheck_timing_backend=upstream-no-assembly-zero-fallback'
  fi
} >capability-inventory.txt

{
  printf 'suite=%s\n' "$suite"
  printf 'driver=%s\n' "$suite_driver"
  printf 'make_target=%s\n' "$suite_make_target"
  printf 'primary_tcl_entrypoint=%s\n' "$suite_tcl_entrypoint"
  printf 'components=%s\n' "$suite_components"
  printf 'command=%s\n' "$suite_command"
  if [[ "$suite" == full ]]; then
    printf '%s\n' \
      'fuzzcheck_amalgamation_language_mode=gnu11' \
      'fuzzcheck_support_language_mode=gnu11' \
      'fuzzcheck_hwtime_predicate_override=__STRICT_ANSI__=1' \
      'fuzzcheck_timing_backend=upstream-no-assembly-fallback' \
      'other_translation_language_mode=gnu11'
    printf 'fuzzcheck_translation_units=%s\n' \
      "$expected_fuzzcheck_translation_units"
    printf 'fuzzcheck_support_translation_units=%s\n' \
      "$expected_fuzzcheck_support_translation_units"
  fi
} >suite-plan.txt

export CCC_SQLITE_COMMAND_LOG="$build_directory/compile-commands.txt"
export CCC_SQLITE_SOURCE_ROOT="$source_directory"
export CCC_SQLITE_GENERATED_SOURCE_ROOT="$build_directory"
export CCC_SQLITE_SOURCE_LOG="$build_directory/source-inputs.txt"
export CCC_SQLITE_LANGUAGE_MODE_LOG="$build_directory/language-modes.txt"
export CCC_SQLITE_FUZZCHECK_HWTIME_FALLBACK=1
: >"$CCC_SQLITE_COMMAND_LOG"
: >"$CCC_SQLITE_SOURCE_LOG"
: >"$CCC_SQLITE_LANGUAGE_MODE_LOG"

{
  make -j"$jobs" BCC="$CCC_LINK_CC -g" \
    CC="$script_directory/ccc-cc" testfixture
} 2>&1 | tee build-run.log

post_build_generated_sha3=$(openssl dgst -sha3-256 sqlite3.c | awk '{print $NF}')
[[ "$post_build_generated_sha3" == "$expected_generated_sha3" ]] ||
  die "generated sqlite3.c changed after applying the test-source adjustment"

actual_source_inputs="$build_directory/testfixture-source-inputs.txt"
LC_ALL=C sort "$CCC_SQLITE_SOURCE_LOG" >"$actual_source_inputs"
actual_source_count=$(wc -l <"$actual_source_inputs" | tr -d '[:space:]')
[[ "$actual_source_count" == "$expected_testfixture_translation_units" ]] ||
  die "CCC translated $actual_source_count testfixture C inputs; expected $expected_testfixture_translation_units"
unique_source_count=$(LC_ALL=C sort -u "$actual_source_inputs" | wc -l | tr -d '[:space:]')
[[ "$unique_source_count" == "$actual_source_count" ]] ||
  die "SQLite testfixture translated at least one C input more than once"
cmp -s "$expected_source_inputs" "$actual_source_inputs" ||
  die "SQLite testfixture C inputs differ from the configured upstream source set"

ccc_command_count=$(grep -c '^ccc ' "$CCC_SQLITE_COMMAND_LOG" || true)
[[ "$ccc_command_count" == "$expected_testfixture_translation_units" ]] ||
  die "SQLite command log contains $ccc_command_count CCC translations; expected $expected_testfixture_translation_units"
link_command_count=$(grep -c '^link ' "$CCC_SQLITE_COMMAND_LOG" || true)
[[ "$link_command_count" == 1 ]] ||
  die "SQLite testfixture build used $link_command_count native link commands; expected 1"
if grep -Eq '^(ccc|link) .* -no-pie( |$)' "$CCC_SQLITE_COMMAND_LOG"; then
  die "SQLite build unexpectedly disabled PIE"
fi
if grep -Eq '^link .*\.(c|i)( |$)' "$CCC_SQLITE_COMMAND_LOG"; then
  die "SQLite native link received a C or preprocessed-C input"
fi
verify_pie_executable testfixture elf-headers.txt elf-dynamic-tags.txt

{
  run_sqlite_suite "$source_directory" "$CCC_LINK_CC" \
    "$script_directory/ccc-cc"
} 2>&1 | tee test-run.log

[[ -f test-out.txt ]] || {
  echo "SQLite test output was not produced" >&2
  exit 1
}

final_generated_sha3=$(openssl dgst -sha3-256 sqlite3.c | awk '{print $NF}')
[[ "$final_generated_sha3" == "$expected_generated_sha3" ]] ||
  die "generated sqlite3.c changed while running the selected suite"
if grep -Eq '^link .*\.(c|i)( |$)' "$CCC_SQLITE_COMMAND_LOG"; then
  die "a suite-added native link received C or preprocessed-C input"
fi

if [[ "$suite" == full ]]; then
  verify_pie_executable \
    fuzzcheck fuzzcheck-elf-headers.txt fuzzcheck-elf-dynamic-tags.txt
  verify_pie_executable \
    sessionfuzz sessionfuzz-elf-headers.txt sessionfuzz-elf-dynamic-tags.txt
fi

hwtime_override_translation_count=$(grep -c \
  '^gnu11 fuzzcheck-amalgamation strict-ansi=defined ' \
  "$CCC_SQLITE_LANGUAGE_MODE_LOG" || true)
fuzzcheck_support_translation_count=$(grep -c \
  '^gnu11 fuzzcheck-support strict-ansi=absent ' \
  "$CCC_SQLITE_LANGUAGE_MODE_LOG" || true)
unexpected_language_mode_count=$(grep -Evc \
  '^(gnu11 ordinary strict-ansi=absent|gnu11 fuzzcheck-support strict-ansi=absent|gnu11 fuzzcheck-amalgamation strict-ansi=defined) ' \
  "$CCC_SQLITE_LANGUAGE_MODE_LOG" || true)
[[ "$unexpected_language_mode_count" == 0 ]] ||
  die "SQLite language-mode audit contains $unexpected_language_mode_count unexpected records"
if [[ "$suite" == full ]]; then
  [[ "$hwtime_override_translation_count" == 1 ]] ||
    die "SQLite full suite applied the hwtime predicate override to $hwtime_override_translation_count inputs; expected the generated amalgamation only"
  [[ "$fuzzcheck_support_translation_count" == "$expected_fuzzcheck_support_translation_units" ]] ||
    die "SQLite full suite translated $fuzzcheck_support_translation_count fuzzcheck support inputs; expected $expected_fuzzcheck_support_translation_units in GNU C11"
  [[ "$((hwtime_override_translation_count + fuzzcheck_support_translation_count))" == "$expected_fuzzcheck_translation_units" ]] ||
    die "SQLite full suite fuzzcheck source audit did not cover $expected_fuzzcheck_translation_units inputs"
else
  [[ "$hwtime_override_translation_count" == 0 ]] ||
    die "SQLite $suite suite unexpectedly translated a fuzzcheck input"
fi

printf 'SQLite %s %s artifacts: %s\n' "$version" "$suite" "$build_directory"
