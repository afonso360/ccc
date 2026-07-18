#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=$(cd "$script_directory/../.." && pwd)
manifest="$script_directory/manifest.toml"
source "$repository/test-corpus/adapter-environment.sh"
source "$script_directory/source-adjustment.sh"

manifest_string() {
  sed -n "s/^$1 = \"\(.*\)\"$/\1/p" "$manifest"
}

manifest_integer() {
  sed -n "s/^$1 = \([0-9][0-9]*\)$/\1/p" "$manifest"
}

usage() {
  echo "usage: $0 [--source-archive PATH] [--work-dir PATH] [--jobs COUNT]" >&2
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
  local path=$1
  local expected_bytes=$2
  local expected_sha256=$3
  local expected_sha3=$4
  local actual_bytes actual_sha256 actual_sha3

  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$(openssl dgst -sha256 "$path" | awk '{print $NF}')
  actual_sha3=$(openssl dgst -sha3-256 "$path" | awk '{print $NF}')
  [[ "$actual_bytes" == "$expected_bytes" ]] ||
    die "zstd archive size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] ||
    die "zstd archive SHA-256 mismatch"
  [[ "$actual_sha3" == "$expected_sha3" ]] ||
    die "zstd archive SHA3-256 mismatch"
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

archive=${ZSTD_SOURCE_ARCHIVE:-}
work_directory=${ZSTD_WORK_DIR:-}
jobs=${ZSTD_BUILD_JOBS:-2}

while (($#)); do
  case "$1" in
    --source-archive)
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
  echo "zstd build job count must be a positive integer: $jobs" >&2
  exit 2
}

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
  die "zstd execution validation requires x86-64 Linux"
export LC_ALL=C
export TZ=UTC
umask 022

for tool in awk basename cat cmp cp dd diff dirname file find gcc grep id make md5sum mkdir mkfifo mktemp mv openssl patch readelf rm sed sort stat tar tee touch tr uname wc; do
  require_tool "$tool"
done

if [[ "$(id -u)" == 0 ]]; then
  die "zstd's upstream smoke tests must run as a non-root user so permission checks are meaningful"
fi

version=$(manifest_string version)
origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_bytes=$(manifest_integer archive_bytes)
expected_sha256=$(manifest_string archive_sha256)
expected_sha3=$(manifest_string archive_sha3_256)
adjustment_patch=$(manifest_string source_adjustment_patch)
expected_adjustment_sha256=$(manifest_string source_adjustment_sha256)
adjustment_hashes=$(manifest_string source_adjustment_hashes)
expected_adjustment_hashes_sha256=$(manifest_string source_adjustment_hashes_sha256)
adjustment_target_files=$(manifest_integer source_adjustment_target_files)
adjustment_rationale=$(manifest_string source_adjustment_rationale)
expected_translation_occurrences=$(manifest_integer source_translation_occurrences)
expected_probe_translation_occurrences=$(manifest_integer generated_pthread_probe_translation_occurrences)
expected_probe_sha256=$(manifest_string generated_pthread_probe_sha256)
expected_link_commands=$(manifest_integer native_link_commands)
expected_probe_link_commands=$(manifest_integer generated_pthread_probe_link_commands)
success_marker=$(manifest_string success_marker)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-zstd-$version.XXXXXX")
else
  if [[ -e "$work_directory" ]] && [[ ! -d "$work_directory" ]]; then
    echo "zstd work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "zstd work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

cache_directory=${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/zstd
if [[ -z "$archive" ]]; then
  mkdir -p "$cache_directory"
  archive="$cache_directory/$archive_name"
  download_archive "$origin" "$archive"
fi
archive=$(absolute_file "$archive")
verify_archive "$archive" "$expected_bytes" "$expected_sha256" "$expected_sha3"

source_parent="$work_directory/source"
mkdir -p "$source_parent"
tar -xzf "$archive" -C "$source_parent"
source_directory="$source_parent/zstd-$version"
[[ -d "$source_directory/lib" && -d "$source_directory/programs" &&
  -d "$source_directory/tests" ]] || die "zstd source archive has an unexpected layout"

grep -Eq '^#define ZSTD_VERSION_MAJOR[[:space:]]+1$' "$source_directory/lib/zstd.h" ||
  die "zstd source major version does not match the corpus pin"
grep -Eq '^#define ZSTD_VERSION_MINOR[[:space:]]+5$' "$source_directory/lib/zstd.h" ||
  die "zstd source minor version does not match the corpus pin"
grep -Eq '^#define ZSTD_VERSION_RELEASE[[:space:]]+7$' "$source_directory/lib/zstd.h" ||
  die "zstd source patch version does not match the corpus pin"
grep -Fq 'Redistribution and use in source and binary forms' "$source_directory/LICENSE" ||
  die "zstd BSD license text is missing"
grep -Fq 'GNU GENERAL PUBLIC LICENSE' "$source_directory/COPYING" ||
  die "zstd GPL license text is missing"

hash_target_count=$(grep -Evc '^[[:space:]]*(#|$)' \
  "$script_directory/adjustments/$adjustment_hashes")
[[ "$hash_target_count" == "$adjustment_target_files" ]] ||
  die "zstd source-adjustment target count does not match the corpus pin"
apply_zstd_source_adjustment \
  "$source_directory" \
  "$work_directory" \
  "$script_directory/adjustments/$adjustment_patch" \
  "$expected_adjustment_sha256" \
  "$script_directory/adjustments/$adjustment_hashes" \
  "$expected_adjustment_hashes_sha256" \
  "$adjustment_rationale"

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_LINK_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
export CCC CCC_RESOURCE_DIR CCC_LINK_CC
export CCC_ZSTD_COMMAND_LOG="$work_directory/compile-commands.txt"

record_native_gcc_driver \
  zstd "$CCC_LINK_CC" \
  "$work_directory/link-driver-identity.txt" \
  "$work_directory/link-driver-macros.txt"

clear_ambient_make_injection
unset CC CFLAGS CPPFLAGS LDFLAGS LDLIBS LIBS
unset MOREFLAGS ASFLAGS DEBUGFLAGS
unset ZSTD_CLEVEL ZSTD_NBTHREADS ZSTD_NOCOMPRESS ZSTD_NODICTID
unset BACKTRACE DATAGEN_BIN DIFF EXE_PREFIX GREP OS PYTHON QEMU_SYS
unset TESTFLAGS UNAME ZSTD_BIN isTerminal size

: >"$CCC_ZSTD_COMMAND_LOG"
"$script_directory/ccc-cc" -DZSTD_DISABLE_ASM \
  -DMEM_FORCE_MEMORY_ACCESS=0 -DXXH_FORCE_MEMORY_ACCESS=0 \
  -dM -E "$script_directory/predicate-probe.c" \
  >"$work_directory/effective-macros.txt"
"$script_directory/ccc-cc" -DZSTD_DISABLE_ASM \
  -DMEM_FORCE_MEMORY_ACCESS=0 -DXXH_FORCE_MEMORY_ACCESS=0 \
  -P -E "$script_directory/predicate-probe.c" \
  >"$work_directory/predicate-probe.txt"

for macro in \
  '#define __GNUC__ 4' \
  '#define __GNUC_MINOR__ 2' \
  '#define __GNUC_PATCHLEVEL__ 1' \
  '#define __x86_64__ 1' \
  '#define __LP64__ 1' \
  '#define ZSTD_DISABLE_ASM 1' \
  '#define MEM_FORCE_MEMORY_ACCESS 0' \
  '#define XXH_FORCE_MEMORY_ACCESS 0' \
  '#define _GNU_SOURCE' \
  '#define __USE_GNU 1' \
  '#define __USE_MISC 1' \
  '#define __USE_XOPEN2K8 1'; do
  grep -Fxq "$macro" "$work_directory/effective-macros.txt" ||
    die "CCC does not expose zstd's pinned compiler configuration: $macro"
done
if grep -Fq '#define __STRICT_ANSI__' "$work_directory/effective-macros.txt"; then
  die "zstd compiler profile unexpectedly selected strict header mode"
fi
for selection in \
  'gnu_compatibility_tuple=4.2.1' \
  'selected_data_model=x86_64-lp64' \
  'selected_builtin=__builtin_expect' \
  'count_bit_builtin_registry=clz-clzll-ctz-ctzll' \
  'additional_builtin_registry=bswap64-prefetch-only' \
  'selected_assembly=disabled' \
  'selected_count_bits=gnu-builtins' \
  'selected_prefetch=compiler-builtin' \
  'selected_zstd_unaligned_access=memcpy' \
  'selected_xxhash_unaligned_access=memcpy' \
  'selected_memory_dependencies=compiler-builtins' \
  'selected_assert=system-gnu-macro' \
  'selected_host_features=glibc-gnu'; do
  grep -Fxq "$selection" "$work_directory/predicate-probe.txt" ||
    die "CCC does not select zstd's pinned source path: $selection"
done
{
  grep -E '^(selected_|count_bit_|additional_builtin_)' \
    "$work_directory/predicate-probe.txt"
  printf '%s\n' \
    'optional_format_library=zlib-disabled' \
    'optional_format_library=lzma-disabled' \
    'optional_format_library=lz4-disabled' \
    'threading=pthread-enabled' \
    'unaligned_memory_access=upstream-memcpy-fallbacks' \
    'memory_dependencies=compiler-builtins' \
    'advertised_but_unselected_builtin=__builtin_bswap64' \
    'unavailable_source_spelling=__builtin_altivec_vmuleuw' \
    'unavailable_source_spelling=__builtin_altivec_vmulouw' \
    'unavailable_source_spelling=__builtin_assume' \
    'unavailable_source_spelling=__builtin_bswap32' \
    'unavailable_source_spelling=__builtin_rotateleft32' \
    'unavailable_source_spelling=__builtin_rotateleft64' \
    'unavailable_source_spelling=__builtin_unreachable' \
    'wide_integer_use=none' \
    'variable_length_array_object=none' \
    'statement_expression=none' \
    'computed_goto=none'
} >"$work_directory/capability-inventory.txt"

expected_source_inputs="$work_directory/expected-source-inputs.txt"
raw_expected_source_inputs="$work_directory/expected-source-inputs.raw"
ALREADY_APPENDED_NOEXECSTACK=1 make \
  -C "$source_directory/programs" \
  --no-print-directory -s \
  -f Makefile -f "$script_directory/source-set.mk" \
  CC=false ZSTD_NO_ASM=1 ZSTD_LEGACY_SUPPORT=5 \
  HAVE_PTHREAD=1 HAVE_ZLIB=0 HAVE_LZMA=0 HAVE_LZ4=0 ALIGN_LOOP= \
  ccc-zstd-source-set >"$raw_expected_source_inputs"
while IFS= read -r source; do
  absolute_file "$source"
done <"$raw_expected_source_inputs" >"$expected_source_inputs.unsorted"
for source in \
  "$source_directory/programs/datagen.c" \
  "$source_directory/programs/lorem.c" \
  "$source_directory/tests/loremOut.c" \
  "$source_directory/tests/datagencli.c"; do
  absolute_file "$source" >>"$expected_source_inputs.unsorted"
done
pthread_probe="$source_directory/programs/have_pthread.c"
for ((probe = 0; probe < expected_probe_translation_occurrences; probe++)); do
  printf '%s\n' "$pthread_probe" >>"$expected_source_inputs.unsorted"
done
LC_ALL=C sort "$expected_source_inputs.unsorted" >"$expected_source_inputs"
rm "$expected_source_inputs.unsorted" "$raw_expected_source_inputs"
actual_expected_occurrences=$(wc -l <"$expected_source_inputs" | tr -d '[:space:]')
[[ "$actual_expected_occurrences" == "$expected_translation_occurrences" ]] ||
  die "zstd source archive selects $actual_expected_occurrences C translations; expected $expected_translation_occurrences"

: >"$CCC_ZSTD_COMMAND_LOG"
export CCC_ZSTD_SOURCE_ROOT="$source_directory"
export CCC_ZSTD_SOURCE_LOG="$work_directory/source-inputs.txt"
: >"$CCC_ZSTD_SOURCE_LOG"
export CCC_ZSTD_PTHREAD_PROBE="$pthread_probe"
export CCC_ZSTD_PTHREAD_PROBE_SHA256="$expected_probe_sha256"
export CCC_ZSTD_PROBE_HASH_LOG="$work_directory/generated-probe-hashes.txt"
: >"$CCC_ZSTD_PROBE_HASH_LOG"
export ALREADY_APPENDED_NOEXECSTACK=1
make -C "$source_directory" -j"$jobs" check \
  CC="$script_directory/ccc-cc" \
  V=1 \
  DEBUGLEVEL=2 \
  ZSTD_NO_ASM=1 \
  ZSTD_LEGACY_SUPPORT=5 \
  HAVE_PTHREAD=1 \
  HAVE_ZLIB=0 \
  HAVE_LZMA=0 \
  HAVE_LZ4=0 \
  ALIGN_LOOP= \
  MOREFLAGS="-DMEM_FORCE_MEMORY_ACCESS=0 -DXXH_FORCE_MEMORY_ACCESS=0" \
  2>&1 | tee "$work_directory/build-test.log"

grep -Fq "$success_marker" "$work_directory/build-test.log" ||
  die "zstd upstream smoke suite did not print its success marker"
[[ -x "$source_directory/programs/zstd" ]] || die "zstd executable was not produced"
[[ -x "$source_directory/tests/datagen" ]] || die "zstd datagen test executable was not produced"

actual_translation_occurrences=$(grep -c '^ccc ' "$CCC_ZSTD_COMMAND_LOG" || true)
[[ "$actual_translation_occurrences" == "$expected_translation_occurrences" ]] ||
  die "zstd build translated $actual_translation_occurrences C inputs; expected $expected_translation_occurrences"
LC_ALL=C sort "$CCC_ZSTD_SOURCE_LOG" >"$CCC_ZSTD_SOURCE_LOG.sorted"
mv "$CCC_ZSTD_SOURCE_LOG.sorted" "$CCC_ZSTD_SOURCE_LOG"
cmp -s "$expected_source_inputs" "$CCC_ZSTD_SOURCE_LOG" ||
  die "zstd build did not translate the exact pinned multiset of C source files"
actual_probe_translations=$(grep -Fc "$expected_probe_sha256  $pthread_probe" \
  "$CCC_ZSTD_PROBE_HASH_LOG" || true)
[[ "$actual_probe_translations" == "$expected_probe_translation_occurrences" ]] ||
  die "zstd build did not translate the exact generated pthread probes"
if grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | grep -Eq '\.(c|i)( |$)'; then
  die "zstd native link command received a C source input"
fi
actual_link_commands=$(grep -c '^link ' "$CCC_ZSTD_COMMAND_LOG" || true)
[[ "$actual_link_commands" == "$expected_link_commands" ]] ||
  die "zstd build invoked $actual_link_commands native links; expected $expected_link_commands"
actual_probe_link_commands=$(grep '^link ' "$CCC_ZSTD_COMMAND_LOG" |
  grep -c -- ' -o have_pthread ' || true)
[[ "$actual_probe_link_commands" == "$expected_probe_link_commands" ]] ||
  die "zstd build invoked $actual_probe_link_commands pthread probe links; expected $expected_probe_link_commands"
if grep '^link ' "$CCC_ZSTD_COMMAND_LOG" | \
  grep -Eq -- ' -pie( |$)| -no-pie( |$)'; then
  die "zstd native links unexpectedly overrode the platform PIE default"
fi
explicit_standard_translations=$(grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | \
  grep -Ec -- ' -std=' || true)
[[ "$explicit_standard_translations" == 0 ]] ||
  die "zstd C translations unexpectedly overrode CCC's default GNU language mode"
for option in \
  ' -DZSTD_DISABLE_ASM' \
  ' -DMEM_FORCE_MEMORY_ACCESS=0' \
  ' -DXXH_FORCE_MEMORY_ACCESS=0'; do
  matches=$(grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -c -- "$option" || true)
  [[ "$matches" == "$expected_translation_occurrences" ]] ||
    die "zstd C translations did not all retain the pinned option:$option"
done
if grep '^ccc ' "$CCC_ZSTD_COMMAND_LOG" | grep -q -- ' -U__GNUC__'; then
  die "zstd C translations suppressed CCC's advertised compiler identity"
fi

: >"$work_directory/elf-headers.txt"
: >"$work_directory/elf-dynamic-tags.txt"
for executable in programs/zstd tests/datagen; do
  binary="$source_directory/$executable"
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
    die "zstd $executable is $elf_type rather than the required PIE executable type"
  readelf --dynamic "$binary" | grep -Eq '\(FLAGS_1\).*PIE' ||
    die "zstd $executable does not carry the PIE dynamic flag"
  if readelf --dynamic "$binary" | grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL'; then
    die "zstd $executable contains dynamic text relocations"
  fi
done

zstd_binary="$source_directory/programs/zstd"
"$zstd_binary" -V 2>&1 | tee "$work_directory/version.log"
grep -Fq "v$version" "$work_directory/version.log" ||
  die "built zstd executable reports an unexpected version"

roundtrip_input="$work_directory/roundtrip-input.txt"
roundtrip_compressed="$work_directory/roundtrip-compressed.zst"
roundtrip_output="$work_directory/roundtrip-output.txt"
roundtrip_stream_output="$work_directory/roundtrip-stream-output.txt"
awk 'BEGIN {
  for (i = 0; i < 16384; ++i) {
    printf "record=%05d lane=%02d checksum=%08x payload=ccc-zstd-roundtrip-%d-%d\n", i, i % 31, (i * 2654435761) % 4294967296, i % 97, i % 13
  }
}' >"$roundtrip_input"
{
  "$zstd_binary" -q -T1 -f "$roundtrip_input" -o "$roundtrip_compressed"
  "$zstd_binary" -q -t "$roundtrip_compressed"
  "$zstd_binary" -q -d -f "$roundtrip_compressed" -o "$roundtrip_output"
  cmp "$roundtrip_input" "$roundtrip_output"
  "$zstd_binary" -q -T1 -c "$roundtrip_input" |
    "$zstd_binary" -q -d -c >"$roundtrip_stream_output"
  cmp "$roundtrip_input" "$roundtrip_stream_output"
  printf 'zstd deterministic file and stream round trips passed\n'
} 2>&1 | tee "$work_directory/roundtrip.log"

grep -Fxq 'zstd deterministic file and stream round trips passed' \
  "$work_directory/roundtrip.log" || die "zstd round-trip checks did not complete"

printf 'zstd %s test artifacts: %s\n' "$version" "$work_directory"
