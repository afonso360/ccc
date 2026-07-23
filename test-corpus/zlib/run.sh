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
    die "zlib archive size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] || die "zlib archive SHA-256 mismatch"
  [[ "$actual_sha3" == "$expected_sha3" ]] || die "zlib archive SHA3-256 mismatch"
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

source_archive=${ZLIB_SOURCE_ARCHIVE:-}
work_directory=${ZLIB_WORK_DIR:-}
jobs=${ZLIB_BUILD_JOBS:-2}

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
  echo "zlib build job count must be a positive integer: $jobs" >&2
  exit 2
}

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
  die "zlib execution validation requires x86-64 Linux"

for tool in ar awk bash cmp cp dirname find gcc grep make mkdir mktemp mv openssl readelf rm sed sort tar tee tr uname wc; do
  require_tool "$tool"
done

version=$(manifest_string version)
origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_bytes=$(manifest_integer archive_bytes)
expected_sha256=$(manifest_string archive_sha256)
expected_sha3=$(manifest_string archive_sha3_256)
expected_occurrences=$(manifest_integer expected_translation_occurrences)
expected_core_units=$(manifest_integer core_translation_units)

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-zlib-$version.XXXXXX")
else
  if [[ -e "$work_directory" && ! -d "$work_directory" ]]; then
    echo "zlib work path is not a directory: $work_directory" >&2
    exit 2
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "zlib work directory must be empty: $work_directory" >&2
    exit 2
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

cache_directory=${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/zlib
if [[ -z "$source_archive" ]]; then
  mkdir -p "$cache_directory"
  source_archive="$cache_directory/$archive_name"
  download_archive "$origin" "$source_archive"
fi
source_archive=$(absolute_file "$source_archive")
verify_archive "$source_archive" "$expected_bytes" "$expected_sha256" "$expected_sha3"

source_parent="$work_directory/source"
mkdir -p "$source_parent"
tar -xzf "$source_archive" -C "$source_parent"
source_directory="$source_parent/zlib-$version"
[[ -x "$source_directory/configure" && -f "$source_directory/Makefile.in" ]] ||
  die "zlib source archive has an unexpected layout"
grep -Fq "zlib $version" "$source_directory/README" ||
  die "zlib source version does not match the corpus pin"
grep -Fq 'Permission is granted to anyone to use this software for any purpose' \
  "$source_directory/LICENSE" || die "zlib license text is missing"

: "${CCC:=$repository/target/debug/ccc}"
: "${CCC_RESOURCE_DIR:=$repository/resource-dir}"
: "${CCC_CC:=gcc}"
CCC=$(resolve_executable "$CCC")
CCC_RESOURCE_DIR=$(absolute_directory "$CCC_RESOURCE_DIR")
CCC_CC=$(resolve_executable "$CCC_CC")
export CCC_RESOURCE_DIR CCC_CC

record_native_gcc_driver \
  zlib "$CCC_CC" \
  "$work_directory/link-driver-identity.txt" \
  "$work_directory/link-driver-macros.txt"

clear_ambient_make_injection
unset CFLAGS CPPFLAGS LDFLAGS LIBS ARFLAGS

"$CCC" -v >"$work_directory/ccc-version.txt" 2>&1
grep -Fqi 'gcc-compatible profile 4.2.1' "$work_directory/ccc-version.txt" ||
  die "CCC verbose identity does not expose the selected GNU compatibility profile"

(
  cd "$source_directory"
  CC="$CCC" ./configure
) 2>&1 | tee "$work_directory/configure-output.log"
cp "$source_directory/configure.log" "$work_directory/configure.log"

grep -Fq "Building shared library libz.so.$version with $CCC." \
  "$work_directory/configure-output.log" ||
  die "zlib configure did not select CCC shared-library support"
grep -Fxq "CC=$CCC" "$source_directory/Makefile" ||
  die "zlib generated Makefile does not retain CCC as CC"
grep -Eq '^CFLAGS=-O3 -fPIC( -DHAVE_HIDDEN)?$' "$source_directory/Makefile" ||
  die "zlib configure selected unexpected compiler flags"

(
  cd "$source_directory"
  make -j"$jobs" test test64
) 2>&1 | tee "$work_directory/build.log"

for marker in \
  '*** zlib test OK ***' \
  '*** zlib shared test OK ***' \
  '*** zlib 64-bit test OK ***'; do
  grep -Fq "$marker" "$work_directory/build.log" ||
    die "zlib upstream tests did not print success marker: $marker"
done
grep -F '*** zlib' "$work_directory/build.log" >"$work_directory/test.log"

for output in \
  libz.a "libz.so.$version" \
  example minigzip example64 minigzip64 examplesh minigzipsh; do
  [[ -f "$source_directory/$output" ]] || die "zlib did not produce $output"
done

ar t "$source_directory/libz.a" >"$work_directory/archive-members.txt"
archive_members=$(wc -l <"$work_directory/archive-members.txt" | tr -d '[:space:]')
[[ "$archive_members" == "$expected_core_units" ]] ||
  die "zlib static archive has $archive_members members; expected $expected_core_units"

: >"$work_directory/elf-headers.txt"
: >"$work_directory/dynamic-tags.txt"
for executable in example minigzip example64 minigzip64 examplesh minigzipsh; do
  readelf --file-header "$source_directory/$executable" >>"$work_directory/elf-headers.txt"
  readelf --dynamic "$source_directory/$executable" >>"$work_directory/dynamic-tags.txt"
  elf_type=$(readelf --file-header "$source_directory/$executable" |
    awk '/^[[:space:]]*Type:/{print $2; exit}')
  [[ "$elf_type" == DYN ]] || die "zlib $executable is not a PIE executable"
  readelf --dynamic "$source_directory/$executable" | grep -Eq '\(FLAGS_1\).*PIE' ||
    die "zlib $executable does not carry the PIE dynamic flag"
done

shared="$source_directory/libz.so.$version"
readelf --file-header "$shared" >>"$work_directory/elf-headers.txt"
readelf --dynamic "$shared" >>"$work_directory/dynamic-tags.txt"
[[ "$(readelf --file-header "$shared" | awk '/^[[:space:]]*Type:/{print $2; exit}')" == DYN ]] ||
  die "zlib shared library is not an ELF dynamic object"
readelf --dynamic "$shared" | grep -Fq 'Library soname: [libz.so.1]' ||
  die "zlib shared library has an unexpected SONAME"
if grep -Eq '\(TEXTREL\)|FLAGS.*TEXTREL' "$work_directory/dynamic-tags.txt"; then
  die "zlib outputs contain dynamic text relocations"
fi

awk -v compiler="$CCC" '
  index($0, compiler " ") == 1 {
    source = ""
    for (index = 1; index <= NF; ++index) {
      if ($index ~ /\.c$/) source = $index
    }
    if (source != "" && $0 ~ /(^|[[:space:]])-c([[:space:]]|$)/) print source
  }
' "$work_directory/build.log" | sed "s#^$source_directory/##" |
  LC_ALL=C sort >"$work_directory/source-multiset.txt"

{
  for source in \
    adler32.c crc32.c deflate.c infback.c inffast.c inflate.c inftrees.c \
    trees.c zutil.c compress.c uncompr.c gzclose.c gzlib.c gzread.c gzwrite.c; do
    printf '%s\n%s\n' "$source" "$source"
  done
  printf '%s\n' test/example.c test/example.c test/minigzip.c test/minigzip.c
} | LC_ALL=C sort >"$work_directory/expected-source-multiset.txt"

actual_occurrences=$(wc -l <"$work_directory/source-multiset.txt" | tr -d '[:space:]')
[[ "$actual_occurrences" == "$expected_occurrences" ]] ||
  die "zlib build compiled $actual_occurrences source occurrences; expected $expected_occurrences"
cmp -s "$work_directory/expected-source-multiset.txt" "$work_directory/source-multiset.txt" ||
  die "zlib build did not compile the exact pinned source multiset"
grep -F "$CCC " "$work_directory/build.log" >"$work_directory/compile-commands.txt"

{
  for ((index = 0; index < 4096; ++index)); do
    printf 'ccc-zlib-round-trip-%04d\n' "$index"
  done
} >"$work_directory/round-trip-input.bin"
cp "$work_directory/round-trip-input.bin" "$work_directory/round-trip-output.bin"
"$source_directory/minigzip" "$work_directory/round-trip-output.bin"
"$source_directory/minigzip" -d "$work_directory/round-trip-output.bin.gz"
cmp -s "$work_directory/round-trip-input.bin" "$work_directory/round-trip-output.bin" ||
  die "zlib minigzip round trip changed the payload"

cat >"$work_directory/capability-inventory.txt" <<EOF
compiler=ccc
gnu_compatibility_tuple=4.2.1
position_independent_objects=yes
shared_output=yes
static_archive_input=yes
visibility_attribute=hidden
inline_assembly=none-on-x86-64
EOF

printf 'zlib %s test artifacts: %s\n' "$version" "$work_directory"
