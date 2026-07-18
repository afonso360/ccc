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
  echo "usage: $0 --target TRIPLE [--source-archive PATH] [--test-repository PATH] [--work-dir PATH] [--jobs COUNT]" >&2
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
  local expected_sha512=$5
  local actual_bytes actual_sha256 actual_sha3 actual_sha512

  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$("$openssl_tool" dgst -sha256 "$path" | awk '{print $NF}')
  actual_sha3=$("$openssl_tool" dgst -sha3-256 "$path" | awk '{print $NF}')
  actual_sha512=$("$openssl_tool" dgst -sha512 "$path" | awk '{print $NF}')
  [[ "$actual_bytes" == "$expected_bytes" ]] ||
    die "bzip2 archive size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] ||
    die "bzip2 archive SHA-256 mismatch"
  [[ "$actual_sha3" == "$expected_sha3" ]] ||
    die "bzip2 archive SHA3-256 mismatch"
  [[ "$actual_sha512" == "$expected_sha512" ]] ||
    die "bzip2 archive SHA-512 mismatch"
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

prepare_test_repository() {
  local origin=$1
  local commit=$2
  local supplied=$3
  local cache_root cache partial

  if [[ -n "$supplied" ]]; then
    absolute_directory "$supplied"
    return
  fi

  cache_root=${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}/ccc/corpus/bzip2
  cache="$cache_root/bzip2-tests.git"
  mkdir -p "$cache_root"
  if [[ ! -d "$cache" ]]; then
    partial="$cache.partial.$$"
    rm -rf -- "$partial"
    git init --bare "$partial" >/dev/null
    git -C "$partial" fetch --depth=1 "$origin" "$commit"
    git -C "$partial" update-ref refs/ccc/pinned-test FETCH_HEAD
    mv "$partial" "$cache"
  elif ! git -C "$cache" cat-file -e "$commit^{commit}" 2>/dev/null; then
    git -C "$cache" fetch --depth=1 "$origin" "$commit"
    git -C "$cache" update-ref refs/ccc/pinned-test FETCH_HEAD
  fi
  absolute_directory "$cache"
}

record_gnu_target_driver() {
  local driver=$1
  local expected_target_prefix=$2
  local expected_macro=$3
  local identity_artifact=$4
  local macro_artifact=$5
  local reported_target version identity identity_lower
  local -a sysroot_arguments=()

  reported_target=$(LC_ALL=C "$driver" -dumpmachine) ||
    die "bzip2 target driver did not report its target"
  [[ "$reported_target" == "$expected_target_prefix"* ]] ||
    die "bzip2 target driver reports $reported_target rather than $expected_target_prefix"
  version=$(LC_ALL=C "$driver" -dumpfullversion -dumpversion) ||
    die "bzip2 target driver did not report its version"
  identity=$(LC_ALL=C "$driver" --version) ||
    die "bzip2 target driver did not report its identity"
  if [[ -n "$CCC_BZIP2_SYSROOT" ]]; then
    sysroot_arguments+=("--sysroot=$CCC_BZIP2_SYSROOT")
  fi
  LC_ALL=C "$driver" "${sysroot_arguments[@]}" -dM -E -x c /dev/null \
    >"$macro_artifact" || die "bzip2 target driver macro probe failed"
  identity_lower=$(printf '%s\n' "$identity" | tr '[:upper:]' '[:lower:]')
  [[ "$identity_lower" != *clang* ]] ||
    die "bzip2 GNU/Linux target driver must be GCC rather than Clang"
  [[ "$identity_lower" == *gcc* || "$identity_lower" == *"gnu compiler collection"* ]] ||
    die "bzip2 GNU/Linux target driver is not GCC"
  grep -Eq '^#define __GNUC__[[:space:]]+[0-9]+$' "$macro_artifact" ||
    die "bzip2 target driver does not expose GCC identity macros"
  grep -Eq "^#define $expected_macro([[:space:]]+.*)?$" "$macro_artifact" ||
    die "bzip2 target driver does not expose $expected_macro"
  grep -Eq '^#define __linux__([[:space:]]+.*)?$' "$macro_artifact" ||
    die "bzip2 target driver does not expose the Linux target identity"
  if [[ "$target" == riscv64-unknown-linux-gnu ]]; then
    grep -Eq '^#define __riscv_xlen[[:space:]]+64$' "$macro_artifact" ||
      die "bzip2 RISC-V driver does not target RV64"
    grep -Eq '^#define __riscv_float_abi_double[[:space:]]+1$' "$macro_artifact" ||
      die "bzip2 RISC-V driver does not target the LP64D floating-point ABI"
  fi
  {
    printf 'executable=%s\n' "$driver"
    printf 'target=%s\n' "$reported_target"
    printf 'sysroot=%s\n' "${CCC_BZIP2_SYSROOT:-<driver-default>}"
    printf 'version=%s\n' "$version"
    printf '%s\n%s\n' '--version:' "$identity"
  } >"$identity_artifact"
}

record_apple_target_driver() {
  local driver=$1
  local identity_artifact=$2
  local macro_artifact=$3
  local reported_target identity

  reported_target=$(LC_ALL=C "$driver" -dumpmachine) ||
    die "bzip2 Apple driver did not report its target"
  [[ "$reported_target" == arm64-apple-darwin* ||
    "$reported_target" == aarch64-apple-darwin* ]] ||
    die "bzip2 Apple driver reports $reported_target rather than arm64 Apple Darwin"
  identity=$(LC_ALL=C "$driver" --version) ||
    die "bzip2 Apple driver did not report its identity"
  [[ "$identity" == *"Apple clang"* ]] ||
    die "bzip2 Darwin target driver must be Apple Clang"
  LC_ALL=C "$driver" -isysroot "$CCC_BZIP2_SDKROOT" \
    "-mmacosx-version-min=$CCC_BZIP2_DEPLOYMENT_TARGET" \
    -dM -E -x c /dev/null >"$macro_artifact" ||
    die "bzip2 Apple driver macro probe failed"
  grep -Eq '^#define __APPLE__([[:space:]]+.*)?$' "$macro_artifact" ||
    die "bzip2 Apple driver does not expose the Apple target identity"
  grep -Eq '^#define (__aarch64__|__arm64__)([[:space:]]+.*)?$' "$macro_artifact" ||
    die "bzip2 Apple driver does not expose the arm64 target identity"
  {
    printf 'executable=%s\n' "$driver"
    printf 'target=%s\n' "$reported_target"
    printf 'sdkroot=%s\n' "$CCC_BZIP2_SDKROOT"
    printf 'deployment_target=%s\n' "$CCC_BZIP2_DEPLOYMENT_TARGET"
    printf '%s\n%s\n' '--version:' "$identity"
  } >"$identity_artifact"
}

source_archive=${BZIP2_SOURCE_ARCHIVE:-}
test_repository=${BZIP2_TEST_REPOSITORY:-}
work_directory=${BZIP2_WORK_DIR:-}
jobs=${BZIP2_BUILD_JOBS:-2}
target=${BZIP2_TARGET:-}

while (($#)); do
  case "$1" in
    --target)
      (($# >= 2)) || {
        usage
        exit 2
      }
      target=$2
      shift 2
      ;;
    --source-archive)
      (($# >= 2)) || {
        usage
        exit 2
      }
      source_archive=$2
      shift 2
      ;;
    --test-repository)
      (($# >= 2)) || {
        usage
        exit 2
      }
      test_repository=$2
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
  echo "bzip2 build job count must be a positive integer: $jobs" >&2
  exit 2
}

[[ -n "$target" ]] || {
  usage
  exit 2
}

for tool in awk basename cat chmod cmp cp dirname find git grep ln make mkdir mktemp mv rm sed sort tar tee tr uname wc; do
  require_tool "$tool"
done

host_os=$(uname -s)
host_arch=$(uname -m)
execution_kind=native
expected_driver_prefix=
expected_driver_macro=
expected_elf_machine=
expected_elf_flags=

case "$target" in
  x86_64-unknown-linux-gnu)
    [[ "$host_os" == Linux && "$host_arch" == x86_64 ]] ||
      die "bzip2 x86-64 execution requires an x86-64 Linux host"
    : "${CCC_LINK_CC:=gcc}"
    : "${BZIP2_AR:=ar}"
    : "${BZIP2_RANLIB:=ranlib}"
    : "${BZIP2_READELF:=readelf}"
    expected_driver_prefix=x86_64
    expected_driver_macro=__x86_64__
    expected_elf_machine='Advanced Micro Devices X86-64'
    expected_elf_flags='0x0'
    ;;
  aarch64-unknown-linux-gnu)
    [[ "$host_os" == Linux ]] ||
      die "bzip2 AArch64 GNU execution requires a Linux host"
    : "${CCC_LINK_CC:=aarch64-linux-gnu-gcc}"
    : "${BZIP2_AR:=aarch64-linux-gnu-ar}"
    : "${BZIP2_RANLIB:=aarch64-linux-gnu-ranlib}"
    : "${BZIP2_READELF:=aarch64-linux-gnu-readelf}"
    : "${BZIP2_QEMU:=qemu-aarch64}"
    : "${BZIP2_QEMU_ROOT:?set BZIP2_QEMU_ROOT to the AArch64 runtime root}"
    execution_kind=qemu
    expected_driver_prefix=aarch64-linux-gnu
    expected_driver_macro=__aarch64__
    expected_elf_machine=AArch64
    expected_elf_flags='0x0'
    ;;
  riscv64-unknown-linux-gnu)
    [[ "$host_os" == Linux ]] ||
      die "bzip2 RISC-V GNU execution requires a Linux host"
    : "${CCC_LINK_CC:=riscv64-linux-gnu-gcc}"
    : "${BZIP2_AR:=riscv64-linux-gnu-ar}"
    : "${BZIP2_RANLIB:=riscv64-linux-gnu-ranlib}"
    : "${BZIP2_READELF:=riscv64-linux-gnu-readelf}"
    : "${BZIP2_QEMU:=qemu-riscv64}"
    : "${BZIP2_QEMU_ROOT:?set BZIP2_QEMU_ROOT to the RISC-V runtime root}"
    execution_kind=qemu
    expected_driver_prefix=riscv64-linux-gnu
    expected_driver_macro=__riscv
    expected_elf_machine=RISC-V
    expected_elf_flags='0x5'
    ;;
  aarch64-apple-darwin)
    [[ "$host_os" == Darwin &&
      ("$host_arch" == arm64 || "$host_arch" == aarch64) ]] ||
      die "bzip2 Darwin arm64 execution requires a native arm64 macOS host"
    require_tool xcrun
    : "${CCC_LINK_CC:=/usr/bin/cc}"
    : "${BZIP2_AR:=$(xcrun --find ar)}"
    : "${BZIP2_RANLIB:=$(xcrun --find ranlib)}"
    : "${BZIP2_SDKROOT:=$(xcrun --sdk macosx --show-sdk-path)}"
    : "${BZIP2_DEPLOYMENT_TARGET:=${MACOSX_DEPLOYMENT_TARGET:-11.0}}"
    [[ "$BZIP2_DEPLOYMENT_TARGET" =~ ^[0-9]+([.][0-9]+){1,2}$ ]] ||
      die "invalid bzip2 macOS deployment target: $BZIP2_DEPLOYMENT_TARGET"
    CCC_BZIP2_SDKROOT=$(absolute_directory "$BZIP2_SDKROOT")
    CCC_BZIP2_DEPLOYMENT_TARGET=$BZIP2_DEPLOYMENT_TARGET
    ;;
  *)
    die "unsupported bzip2 target: $target"
    ;;
esac

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${BZIP2_OPENSSL:=openssl}"
if [[ "$target" == aarch64-apple-darwin ]]; then
  : "${BZIP2_MD5SUM:=gmd5sum}"
else
  : "${BZIP2_MD5SUM:=md5sum}"
fi

CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_LINK_CC=$(resolve_executable "$CCC_LINK_CC")
archiver=$(resolve_executable "$BZIP2_AR")
archive_indexer=$(resolve_executable "$BZIP2_RANLIB")
openssl_tool=$(resolve_executable "$BZIP2_OPENSSL")
md5sum_tool=$(resolve_executable "$BZIP2_MD5SUM")

"$openssl_tool" dgst -sha3-256 /dev/null >/dev/null 2>&1 ||
  die "bzip2 requires an OpenSSL implementation with SHA3-256 support"

if [[ "$target" == aarch64-apple-darwin ]]; then
  file_tool=$(resolve_executable file)
  nm_tool=$(resolve_executable nm)
  otool_tool=$(resolve_executable otool)
else
  readelf_tool=$(resolve_executable "$BZIP2_READELF")
  if [[ -n "${BZIP2_SYSROOT:-}" ]]; then
    CCC_BZIP2_SYSROOT=$(absolute_directory "$BZIP2_SYSROOT")
  else
    reported_sysroot=$(LC_ALL=C "$CCC_LINK_CC" --print-sysroot) ||
      die "bzip2 target driver did not report a compiler sysroot"
    if [[ -n "$reported_sysroot" ]]; then
      CCC_BZIP2_SYSROOT=$(absolute_directory "$reported_sysroot")
    elif [[ "$execution_kind" == native ]]; then
      CCC_BZIP2_SYSROOT=
    else
      die "bzip2 cross-target driver reported an empty compiler sysroot"
    fi
  fi
  if [[ "$execution_kind" == qemu ]]; then
    qemu=$(resolve_executable "$BZIP2_QEMU")
    qemu_root=$(absolute_directory "$BZIP2_QEMU_ROOT")
    [[ "$qemu_root" != / ]] ||
      die "bzip2 QEMU runtime root must not be the host root"
  fi
fi

CCC_BZIP2_TARGET=$target
export CCC CCC_RESOURCE_DIR CCC_LINK_CC CCC_BZIP2_TARGET
export CCC_BZIP2_SYSROOT CCC_BZIP2_SDKROOT CCC_BZIP2_DEPLOYMENT_TARGET

version=$(manifest_string version)
source_origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_bytes=$(manifest_integer archive_bytes)
expected_sha256=$(manifest_string archive_sha256)
expected_sha3=$(manifest_string archive_sha3_256)
expected_sha512=$(manifest_string archive_sha512)
test_origin=$(manifest_string test_origin)
test_commit=$(manifest_string test_commit)
test_tree=$(manifest_string test_tree)
expected_archive_sources=$(manifest_integer archive_c_translation_units)
expected_build_sources=$(manifest_integer build_translation_units)
expected_links=$(manifest_integer native_link_commands)
expected_good_streams=$(manifest_integer test_good_streams)
expected_bad_streams=$(manifest_integer test_bad_streams)
expected_test_passes=$(manifest_integer test_pass_markers)
extended_success_marker=$(manifest_string extended_success_marker)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-bzip2-$version.XXXXXX")
else
  if [[ -e "$work_directory" ]] && [[ ! -d "$work_directory" ]]; then
    echo "bzip2 work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "bzip2 work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

{
  printf 'target=%s\n' "$target"
  printf 'host=%s/%s\n' "$host_os" "$host_arch"
  printf 'execution=%s\n' "$execution_kind"
  printf 'compiler=%s\n' "$CCC"
  printf 'link_driver=%s\n' "$CCC_LINK_CC"
  printf 'archiver=%s\n' "$archiver"
  printf 'archive_indexer=%s\n' "$archive_indexer"
  if [[ "$target" == aarch64-apple-darwin ]]; then
    printf 'sdkroot=%s\n' "$CCC_BZIP2_SDKROOT"
    printf 'deployment_target=%s\n' "$CCC_BZIP2_DEPLOYMENT_TARGET"
  else
    printf 'compiler_sysroot=%s\n' "${CCC_BZIP2_SYSROOT:-<driver-default>}"
    printf 'readelf=%s\n' "$readelf_tool"
  fi
  if [[ "$execution_kind" == qemu ]]; then
    printf 'qemu=%s\n' "$qemu"
    printf 'qemu_root=%s\n' "$qemu_root"
  fi
} >"$work_directory/target-profile.txt"

if [[ "$target" == aarch64-apple-darwin ]]; then
  record_apple_target_driver \
    "$CCC_LINK_CC" \
    "$work_directory/link-driver-identity.txt" \
    "$work_directory/link-driver-macros.txt"
else
  record_gnu_target_driver \
    "$CCC_LINK_CC" "$expected_driver_prefix" "$expected_driver_macro" \
    "$work_directory/link-driver-identity.txt" \
    "$work_directory/link-driver-macros.txt"
fi

{
  printf 'compiler=%s\n' "$CCC"
  "$CCC" --version 2>&1
  printf 'archiver=%s\n' "$archiver"
  "$openssl_tool" dgst -sha256 "$archiver"
  printf 'archive_indexer=%s\n' "$archive_indexer"
  "$openssl_tool" dgst -sha256 "$archive_indexer"
  printf 'openssl=%s\n' "$openssl_tool"
  "$openssl_tool" version
  printf 'md5sum=%s\n' "$md5sum_tool"
  "$md5sum_tool" --version 2>&1
  if [[ "$target" == aarch64-apple-darwin ]]; then
    for metadata_tool in "$file_tool" "$nm_tool" "$otool_tool"; do
      printf 'metadata_tool=%s\n' "$metadata_tool"
      "$openssl_tool" dgst -sha256 "$metadata_tool"
    done
  else
    printf 'metadata_tool=%s\n' "$readelf_tool"
    "$readelf_tool" --version 2>&1
  fi
  if [[ "$execution_kind" == qemu ]]; then
    printf 'emulator=%s\n' "$qemu"
    "$qemu" --version 2>&1
  fi
} >"$work_directory/tool-identities.txt"

printf 'bzip2-md5-probe\n' >"$work_directory/md5-probe.txt"
"$md5sum_tool" "$work_directory/md5-probe.txt" \
  >"$work_directory/md5-probe-output.txt" ||
  die "bzip2 MD5 tool probe failed"
grep -Eq '^[[:xdigit:]]{32}  .*/md5-probe[.]txt$' \
  "$work_directory/md5-probe-output.txt" ||
  die "bzip2 requires GNU-compatible md5sum output"
rm "$work_directory/md5-probe.txt" "$work_directory/md5-probe-output.txt"
private_tool_directory="$work_directory/private-tools"
mkdir "$private_tool_directory"
ln -s "$md5sum_tool" "$private_tool_directory/md5sum"
export PATH="$private_tool_directory:$PATH"

cache_directory=${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}/ccc/corpus/bzip2
if [[ -z "$source_archive" ]]; then
  mkdir -p "$cache_directory"
  source_archive="$cache_directory/$archive_name"
  download_archive "$source_origin" "$source_archive"
fi
source_archive=$(absolute_file "$source_archive")
verify_archive "$source_archive" "$expected_bytes" "$expected_sha256" \
  "$expected_sha3" "$expected_sha512"

test_repository=$(prepare_test_repository "$test_origin" "$test_commit" "$test_repository")
git -C "$test_repository" cat-file -e "$test_commit^{commit}" 2>/dev/null ||
  die "bzip2 test repository does not contain the pinned commit"
actual_test_commit=$(git -C "$test_repository" rev-parse "$test_commit^{commit}")
actual_test_tree=$(git -C "$test_repository" show -s --format=%T "$actual_test_commit")
[[ "$actual_test_commit" == "$test_commit" ]] || die "bzip2 test commit mismatch"
[[ "$actual_test_tree" == "$test_tree" ]] || die "bzip2 test tree mismatch"
{
  printf 'origin=%s\n' "$test_origin"
  printf 'commit=%s\n' "$actual_test_commit"
  printf 'tree=%s\n' "$actual_test_tree"
} >"$work_directory/test-repository-identity.txt"

source_parent="$work_directory/source"
test_directory="$work_directory/tests"
mkdir -p "$source_parent" "$test_directory"
tar -xzf "$source_archive" -C "$source_parent"
git -C "$test_repository" archive "$test_commit" | tar -xf - -C "$test_directory"
source_directory="$source_parent/bzip2-$version"
[[ -f "$source_directory/Makefile" ]] || die "bzip2 source archive has an unexpected layout"
[[ -x "$test_directory/run-tests.sh" ]] || die "bzip2 test tree has an unexpected layout"

grep -Fq "bzip2/libbzip2 version $version of 13 July 2019" "$source_directory/README" ||
  die "bzip2 source version does not match the corpus pin"
grep -Fq "bzip2/libbzip2 version $version of 13 July 2019" "$source_directory/LICENSE" ||
  die "bzip2 source license does not match the corpus pin"
grep -Fq 'Six self-tests are run.' "$source_directory/README" ||
  die "bzip2 source does not describe the pinned upstream tests"

archive_source_inputs="$work_directory/archive-source-inputs.txt"
find "$source_directory" -maxdepth 1 -type f -name '*.c' -print |
  LC_ALL=C sort >"$archive_source_inputs"
archive_source_count=$(wc -l <"$archive_source_inputs" | tr -d '[:space:]')
[[ "$archive_source_count" == "$expected_archive_sources" ]] ||
  die "bzip2 archive contains $archive_source_count C inputs; expected $expected_archive_sources"
expected_archive_source_inputs="$work_directory/expected-archive-source-inputs.txt"
for source in \
  blocksort.c \
  bzlib.c \
  bzip2.c \
  bzip2recover.c \
  compress.c \
  crctable.c \
  decompress.c \
  dlltest.c \
  huffman.c \
  mk251.c \
  randtable.c \
  spewG.c \
  unzcrash.c; do
  printf '%s/%s\n' "$source_directory" "$source"
done | LC_ALL=C sort >"$expected_archive_source_inputs"
cmp -s "$expected_archive_source_inputs" "$archive_source_inputs" ||
  die "bzip2 archive does not contain the exact pinned C source inventory"
actual_good_streams=$(find "$test_directory" -type f -name '*.bz2' -print | wc -l | tr -d '[:space:]')
actual_bad_streams=$(find "$test_directory" -type f -name '*.bz2.bad' -print | wc -l | tr -d '[:space:]')
[[ "$actual_good_streams" == "$expected_good_streams" ]] ||
  die "bzip2 test tree contains $actual_good_streams good streams; expected $expected_good_streams"
[[ "$actual_bad_streams" == "$expected_bad_streams" ]] ||
  die "bzip2 test tree contains $actual_bad_streams bad streams; expected $expected_bad_streams"

export CCC_BZIP2_COMMAND_LOG="$work_directory/compile-commands.txt"

clear_ambient_make_injection
unset CFLAGS CPPFLAGS LDFLAGS LIBS ARFLAGS
unset BZIP BZIP2
export LC_ALL=C TZ=UTC

: >"$CCC_BZIP2_COMMAND_LOG"
"$script_directory/ccc-cc" -dM -E \
  "$script_directory/predicate-probe.c" >"$work_directory/effective-macros.txt"
"$script_directory/ccc-cc" -P -E \
  "$script_directory/predicate-probe.c" >"$work_directory/predicate-probe.txt"

grep -Fxq '#define __GNUC__ 4' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC__ value"
grep -Fxq '#define __GNUC_MINOR__ 2' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_MINOR__ value"
grep -Fxq '#define __GNUC_PATCHLEVEL__ 1' "$work_directory/effective-macros.txt" ||
  die "CCC does not advertise the pinned __GNUC_PATCHLEVEL__ value"
case "$target" in
  x86_64-unknown-linux-gnu)
    grep -Eq '^#define __x86_64__([[:space:]]+.*)?$' "$work_directory/effective-macros.txt" ||
      die "CCC does not advertise the selected x86-64 target"
    ;;
  aarch64-unknown-linux-gnu)
    grep -Eq '^#define __aarch64__([[:space:]]+.*)?$' "$work_directory/effective-macros.txt" ||
      die "CCC does not advertise the selected AArch64 target"
    ;;
  riscv64-unknown-linux-gnu)
    grep -Eq '^#define __riscv([[:space:]]+.*)?$' "$work_directory/effective-macros.txt" ||
      die "CCC does not advertise the selected RISC-V target"
    ;;
  aarch64-apple-darwin)
    grep -Eq '^#define __APPLE__([[:space:]]+.*)?$' "$work_directory/effective-macros.txt" ||
      die "CCC does not advertise the selected Apple target"
    grep -Eq '^#define (__aarch64__|__arm64__)([[:space:]]+.*)?$' \
      "$work_directory/effective-macros.txt" ||
      die "CCC does not advertise the selected Darwin arm64 target"
    ;;
esac
for selection in \
  'gnu_compatibility_tuple=4.2.1' \
  'selected_attribute=noreturn' \
  'selected_keyword=__inline__' \
  'selected_integer_type=unsigned-long-long'; do
  grep -Fxq "$selection" "$work_directory/predicate-probe.txt" ||
    die "CCC does not select bzip2's pinned compiler path: $selection"
done

{
  grep -E '^(gnu_compatibility_tuple|selected_)' "$work_directory/predicate-probe.txt"
  printf '%s\n' \
    'builtin=none' \
    'computed_goto=none' \
    'inline_assembly=none' \
    'int128_use=none' \
    'variable_length_array_object=none' \
    'statement_expression=none'
} >"$work_directory/capability-inventory.txt"

: >"$CCC_BZIP2_COMMAND_LOG"
export CCC_BZIP2_SOURCE_ROOT="$source_directory"
export CCC_BZIP2_SOURCE_LOG="$work_directory/source-inputs.txt"
: >"$CCC_BZIP2_SOURCE_LOG"
expected_source_inputs="$work_directory/expected-source-inputs.txt"
for source in \
  blocksort.c \
  bzlib.c \
  bzip2.c \
  bzip2recover.c \
  compress.c \
  crctable.c \
  decompress.c \
  huffman.c \
  randtable.c; do
  printf '%s/%s\n' "$source_directory" "$source"
done | LC_ALL=C sort >"$expected_source_inputs"
expected_source_count=$(wc -l <"$expected_source_inputs" | tr -d '[:space:]')
[[ "$expected_source_count" == "$expected_build_sources" ]] ||
  die "bzip2 adapter expects $expected_source_count C inputs; manifest expects $expected_build_sources"

make -C "$source_directory" -j"$jobs" \
  libbz2.a bzip2 bzip2recover \
  CC="$script_directory/ccc-cc" \
  AR="$archiver" \
  RANLIB="$archive_indexer" \
  CFLAGS='-Wall -Winline -O2 -g -D_FILE_OFFSET_BITS=64' \
  2>&1 | tee "$work_directory/build.log"

[[ -f "$source_directory/libbz2.a" ]] || die "bzip2 static library was not produced"
[[ -x "$source_directory/bzip2" ]] || die "bzip2 executable was not produced"
[[ -x "$source_directory/bzip2recover" ]] || die "bzip2recover executable was not produced"

actual_translation_units=$(grep -c '^ccc ' "$CCC_BZIP2_COMMAND_LOG" || true)
[[ "$actual_translation_units" == "$expected_build_sources" ]] ||
  die "bzip2 build translated $actual_translation_units C inputs; expected $expected_build_sources"
LC_ALL=C sort "$CCC_BZIP2_SOURCE_LOG" >"$CCC_BZIP2_SOURCE_LOG.sorted"
mv "$CCC_BZIP2_SOURCE_LOG.sorted" "$CCC_BZIP2_SOURCE_LOG"
cmp -s "$expected_source_inputs" "$CCC_BZIP2_SOURCE_LOG" ||
  die "bzip2 build did not translate the exact pinned set of C source files"
if grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | grep -Eq '\.(c|i)( |$)'; then
  die "bzip2 native link command received a C source input"
fi
link_commands=$(grep -c '^link ' "$CCC_BZIP2_COMMAND_LOG" || true)
[[ "$link_commands" == "$expected_links" ]] ||
  die "bzip2 build invoked $link_commands native links; expected $expected_links"
if grep '^link ' "$CCC_BZIP2_COMMAND_LOG" | \
  grep -Eq -- ' -pie( |$)| -no-pie( |$)'; then
  die "bzip2 native links unexpectedly overrode the platform PIE default"
fi
explicit_standard_translations=$(grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | \
  grep -Ec -- ' -std=' || true)
[[ "$explicit_standard_translations" == 0 ]] ||
  die "bzip2 C translations unexpectedly overrode CCC's default GNU language mode"
large_file_translations=$(grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -c -- ' -D_FILE_OFFSET_BITS=64' || true)
[[ "$large_file_translations" == "$expected_build_sources" ]] ||
  die "bzip2 C translations did not all use the pinned large-file interface"
if grep '^ccc ' "$CCC_BZIP2_COMMAND_LOG" | grep -Eq -- ' -(march|mabi)='; then
  die "bzip2 C translations unexpectedly selected an architecture or ABI flag"
fi

: >"$work_directory/binary-headers.txt"
: >"$work_directory/binary-dynamic.txt"
: >"$work_directory/binary-load-commands.txt"
: >"$work_directory/binary-relocations.txt"
: >"$work_directory/binary-symbols.txt"
for executable in bzip2 bzip2recover; do
  binary="$source_directory/$executable"
  if [[ "$target" == aarch64-apple-darwin ]]; then
    {
      printf '==> %s <==\n' "$executable"
      "$file_tool" "$binary"
      "$otool_tool" -hv "$binary"
    } >>"$work_directory/binary-headers.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$otool_tool" -l "$binary"
    } >>"$work_directory/binary-load-commands.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$nm_tool" -g "$binary"
    } >>"$work_directory/binary-symbols.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$otool_tool" -rv "$binary"
    } >>"$work_directory/binary-relocations.txt"
    "$file_tool" "$binary" | grep -Fq 'Mach-O 64-bit executable arm64' ||
      die "bzip2 $executable is not a Mach-O arm64 executable"
    "$otool_tool" -hv "$binary" | grep -Eq '(^|[[:space:]])PIE([[:space:]]|$)' ||
      die "bzip2 $executable does not carry the Mach-O PIE flag"
    "$otool_tool" -l "$binary" | grep -Fq 'cmd LC_BUILD_VERSION' ||
      die "bzip2 $executable does not carry LC_BUILD_VERSION"
    "$otool_tool" -l "$binary" | grep -Eq \
      "^[[:space:]]*minos[[:space:]]+$CCC_BZIP2_DEPLOYMENT_TARGET([.]0)?$" ||
      die "bzip2 $executable does not carry the selected deployment target"
    "$nm_tool" -g "$binary" | grep -Eq '(^|[[:space:]])_main$' ||
      die "bzip2 $executable does not expose the expected Darwin entry symbol"
    segment_count=0
    text_segment_count=0
    while read -r segment_name maximum_protection initial_protection; do
      [[ -n "$segment_name" ]] || continue
      ((segment_count += 1))
      [[ "$segment_name" != __TEXT ]] || ((text_segment_count += 1))
      if (((maximum_protection & 6) == 6 || (initial_protection & 6) == 6)); then
        die "bzip2 $executable has a writable and executable Mach-O segment: $segment_name"
      fi
    done < <("$otool_tool" -l "$binary" | awk '
      $1 == "cmd" && $2 == "LC_SEGMENT_64" { inside=1; name=max=init=""; next }
      inside && $1 == "segname" { name=$2; next }
      inside && $1 == "maxprot" { max=$2; next }
      inside && $1 == "initprot" { init=$2; print name, max, init; inside=0 }
    ')
    ((segment_count > 0 && text_segment_count == 1)) ||
      die "bzip2 $executable has an unexpected Mach-O segment table"
    relocation_lines=$("$otool_tool" -rv "$binary" | sed '1d' |
      grep -c '[^[:space:]]' || true)
    [[ "$relocation_lines" == 0 ]] ||
      die "bzip2 $executable retains Mach-O relocation entries"
  else
    {
      printf '==> %s <==\n' "$executable"
      "$readelf_tool" --file-header "$binary"
    } >>"$work_directory/binary-headers.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$readelf_tool" --dynamic "$binary"
    } >>"$work_directory/binary-dynamic.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$readelf_tool" --program-headers "$binary"
    } >>"$work_directory/binary-load-commands.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$readelf_tool" --symbols "$binary"
    } >>"$work_directory/binary-symbols.txt"
    {
      printf '==> %s <==\n' "$executable"
      "$readelf_tool" --relocs "$binary"
    } >>"$work_directory/binary-relocations.txt"
    elf_type=$("$readelf_tool" --file-header "$binary" |
      awk '/^[[:space:]]*Type:/{print $2; exit}')
    elf_machine=$("$readelf_tool" --file-header "$binary" |
      awk '/^[[:space:]]*Machine:/{sub(/^[[:space:]]*Machine:[[:space:]]*/, ""); print; exit}')
    elf_flags=$("$readelf_tool" --file-header "$binary" |
      awk '/^[[:space:]]*Flags:/{for (i=1; i<=NF; i++) if ($i ~ /^0x/) {gsub(/,/, "", $i); print $i; exit}}')
    [[ "$elf_type" == DYN ]] ||
      die "bzip2 $executable is $elf_type rather than the required PIE executable type"
    [[ "$elf_machine" == "$expected_elf_machine" ]] ||
      die "bzip2 $executable has ELF machine $elf_machine rather than $expected_elf_machine"
    [[ "$elf_flags" == "$expected_elf_flags" ]] ||
      die "bzip2 $executable has ELF flags $elf_flags rather than $expected_elf_flags"
    "$readelf_tool" --dynamic "$binary" | grep -Eq '\(FLAGS_1\).*PIE' ||
      die "bzip2 $executable does not carry the PIE dynamic flag"
    if "$readelf_tool" --dynamic "$binary" |
      grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL'; then
      die "bzip2 $executable contains dynamic text relocations"
    fi
    interpreter=$("$readelf_tool" --program-headers "$binary" |
      sed -n 's/.*Requesting program interpreter: \([^]]*\).*/\1/p')
    [[ "$interpreter" == /* ]] ||
      die "bzip2 $executable has no absolute ELF program interpreter"
    if [[ "$execution_kind" == qemu ]]; then
      [[ -e "$qemu_root$interpreter" ]] ||
        die "bzip2 $executable interpreter is absent from the QEMU root: $qemu_root$interpreter"
    else
      [[ -e "$interpreter" ]] ||
        die "bzip2 $executable interpreter is absent from the host: $interpreter"
    fi
  fi
done

if [[ "$execution_kind" == qemu ]]; then
  {
    printf 'CCC_BZIP2_QEMU=%q\n' "$qemu"
    printf 'CCC_BZIP2_QEMU_ROOT=%q\n' "$qemu_root"
  } >"$source_directory/.ccc-qemu-config"
  "$qemu" --version >"$work_directory/qemu-identity.txt"
  for executable in bzip2 bzip2recover; do
    mv "$source_directory/$executable" "$source_directory/$executable.target"
    cp "$script_directory/qemu-launcher" "$source_directory/$executable"
    chmod +x "$source_directory/$executable"
  done
fi

"$source_directory/bzip2" --version \
  </dev/null >/dev/null 2>"$work_directory/version.log"
cat "$work_directory/version.log"
grep -Fq "bzip2, a block-sorting file compressor.  Version $version" \
  "$work_directory/version.log" ||
  die "built bzip2 executable reports an unexpected version"

make -C "$source_directory" -j1 check \
  CC="$script_directory/ccc-cc" \
  AR="$archiver" \
  RANLIB="$archive_indexer" \
  CFLAGS='-Wall -Winline -O2 -g -D_FILE_OFFSET_BITS=64' \
  2>&1 | tee "$work_directory/upstream-test.log"
[[ "$(grep -c '^ccc ' "$CCC_BZIP2_COMMAND_LOG" || true)" == "$expected_build_sources" ]] ||
  die "bzip2 upstream test unexpectedly rebuilt a C input"

round_trip_input="$work_directory/round-trip-input.txt"
round_trip_stream="$work_directory/round-trip-stream.bz2"
round_trip_output="$work_directory/round-trip-output.txt"
cat \
  "$source_directory/LICENSE" \
  "$source_directory/README" \
  "$source_directory/CHANGES" \
  "$source_directory/sample1.ref" \
  "$source_directory/sample2.ref" \
  "$source_directory/sample3.ref" \
  >"$round_trip_input"
{
  echo 'bzip2 -9 -c round-trip-input.txt > round-trip-stream.bz2'
  "$source_directory/bzip2" -9 -c "$round_trip_input" >"$round_trip_stream"
  echo 'bzip2 -t round-trip-stream.bz2'
  "$source_directory/bzip2" -t "$round_trip_stream"
  echo 'bzip2 -d -c round-trip-stream.bz2 > round-trip-output.txt'
  "$source_directory/bzip2" -d -c "$round_trip_stream" >"$round_trip_output"
  echo 'cmp round-trip-input.txt round-trip-output.txt'
  cmp "$round_trip_input" "$round_trip_output"
  echo 'round-trip passed'
} 2>&1 | tee "$work_directory/round-trip.log"
grep -Fxq 'round-trip passed' "$work_directory/round-trip.log" ||
  die "bzip2 deterministic round trip did not complete"

(
  cd "$test_directory"
  ./run-tests.sh \
    --bzip2="$source_directory/bzip2" \
    --without-valgrind
) 2>&1 | tee "$work_directory/extended-test.log"
grep -Fxq "$extended_success_marker" "$work_directory/extended-test.log" ||
  die "bzip2 extended test suite did not print its success marker"
actual_test_passes=$(grep -c '^PASS:' "$work_directory/extended-test.log" || true)
[[ "$actual_test_passes" == "$expected_test_passes" ]] ||
  die "bzip2 extended test suite reported $actual_test_passes passing checks; expected $expected_test_passes"

printf 'bzip2 %s test artifacts: %s\n' "$version" "$work_directory"
