#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repository=$(CDPATH='' cd -- "$script_directory/../.." && pwd -P)
manifest="$script_directory/manifest.toml"
measure_command="$script_directory/measure-command.py"
validate_ppm="$script_directory/validate-ppm.py"
summarize="$script_directory/summarize.py"
collect_object_sections="$script_directory/collect-object-sections.py"

manifest_string() {
  sed -n "s/^$1 = \"\\(.*\\)\"$/\\1/p" "$manifest"
}

manifest_integer() {
  sed -n "s/^$1 = \\([0-9][0-9]*\\)$/\\1/p" "$manifest"
}

usage() {
  cat >&2 <<EOF
usage: $0 [OPTIONS]

Build and measure the pinned C-Ray release with correctness checks enabled.

  --profile NAME          correctness or performance (default: correctness)
  --target TRIPLE         native enabled target (default: inferred from host)
  --source-archive PATH   pinned release archive; disables download
  --work-dir PATH         empty artifact directory (default: retained temporary directory)
  --warmups COUNT         render warmups per executable (profile default)
  --samples COUNT         measured render samples per executable (profile default)
  --ccc PATH              CCC executable (default: target/debug/ccc)
  --resource-dir PATH     CCC resource directory (default: resource-dir)
  --reference-cc PATH     native GCC or Clang reference/link driver
  --sdk-root PATH         macOS SDK root (Darwin default: active macOS SDK)
  --deployment-target V   minimum macOS version (Darwin default: 11.0)
  -h, --help              show this help
EOF
}

die() {
  echo "$*" >&2
  exit 1
}

usage_error() {
  echo "$*" >&2
  usage
  exit 2
}

require_tool() {
  command -v -- "$1" >/dev/null 2>&1 ||
    die "required tool is not available: $1"
}

absolute_directory() {
  [[ -d "$1" ]] || die "directory does not exist: $1"
  (CDPATH='' cd -- "$1" && pwd -P)
}

absolute_file() {
  [[ -f "$1" ]] || die "file does not exist: $1"
  printf '%s/%s\n' \
    "$(CDPATH='' cd -- "$(dirname -- "$1")" && pwd -P)" \
    "$(basename -- "$1")"
}

resolve_executable() {
  local executable=$1
  local resolved
  resolved=$(command -v -- "$executable" 2>/dev/null) ||
    die "executable is not available: $executable"
  [[ -x "$resolved" ]] || die "file is not executable: $resolved"
  if [[ "$resolved" != /* ]]; then
    resolved="$(pwd -P)/$resolved"
  fi
  printf '%s\n' "$resolved"
}

nonnegative_integer() {
  [[ "$2" =~ ^(0|[1-9][0-9]*)$ ]] ||
    usage_error "$1 must be a nonnegative integer: $2"
}

positive_integer() {
  [[ "$2" =~ ^[1-9][0-9]*$ ]] ||
    usage_error "$1 must be a positive integer: $2"
}

hash_file() {
  local algorithm=$1
  local path=$2
  openssl dgst "-$algorithm" "$path" | awk '{print $NF}'
}

verify_file() {
  local label=$1
  local path=$2
  local expected_bytes=$3
  local expected_sha256=$4
  local actual_bytes actual_sha256

  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$(hash_file sha256 "$path")
  [[ "$actual_bytes" == "$expected_bytes" ]] ||
    die "$label size mismatch: expected $expected_bytes, found $actual_bytes"
  [[ "$actual_sha256" == "$expected_sha256" ]] ||
    die "$label SHA-256 mismatch"
}

verify_archive() {
  local path=$1
  local expected_bytes=$2
  local expected_sha256=$3
  local expected_sha3=$4
  local actual_sha3

  verify_file "C-Ray archive" "$path" "$expected_bytes" "$expected_sha256"
  actual_sha3=$(hash_file sha3-256 "$path")
  [[ "$actual_sha3" == "$expected_sha3" ]] ||
    die "C-Ray archive SHA3-256 mismatch"
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

record_command() {
  printf 'LC_ALL=C' >>"$commands"
  printf ' %q' "$@" >>"$commands"
  printf '\n' >>"$commands"
}

measure() {
  local stage=$1
  local label=$2
  local iteration=$3
  local standard_output=$4
  local standard_error=$5
  local result_json="$timing_directory/$stage-$label-$iteration.json"
  shift 5
  record_command "$@"
  "$measure_command" \
    --stage "$stage" \
    --label "$label" \
    --iteration "$iteration" \
    --json "$result_json" \
    --results "$timings" \
    --stdout "$standard_output" \
    --stderr "$standard_error" \
    -- "$@"
}

source_archive=${CRAY_SOURCE_ARCHIVE:-}
work_directory=${CRAY_WORK_DIR:-}
profile=${CRAY_PROFILE:-correctness}
target=${CRAY_TARGET:-}
warmups=${CRAY_WARMUPS:-}
samples=${CRAY_SAMPLES:-}
ccc=${CCC:-"$repository/target/debug/ccc"}
resource_directory=${CCC_RESOURCE_DIR:-"$repository/resource-dir"}
reference_cc=${CRAY_REFERENCE_CC:-}
sdk_root=${CRAY_SDK_ROOT:-}
deployment_target=${CRAY_DEPLOYMENT_TARGET:-11.0}

while (($#)); do
  case "$1" in
    --profile | --target | --source-archive | --work-dir | --warmups | \
      --samples | --ccc | --resource-dir | --reference-cc | --sdk-root | \
      --deployment-target)
      (($# >= 2)) || usage_error "missing value for $1"
      option=$1
      value=$2
      case "$option" in
        --profile) profile=$value ;;
        --target) target=$value ;;
        --source-archive) source_archive=$value ;;
        --work-dir) work_directory=$value ;;
        --warmups) warmups=$value ;;
        --samples) samples=$value ;;
        --ccc) ccc=$value ;;
        --resource-dir) resource_directory=$value ;;
        --reference-cc) reference_cc=$value ;;
        --sdk-root) sdk_root=$value ;;
        --deployment-target) deployment_target=$value ;;
      esac
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage_error "unknown option: $1"
      ;;
  esac
done

case "$profile" in
  correctness | performance) ;;
  *) usage_error "unsupported C-Ray profile: $profile" ;;
esac

host_os=$(uname -s)
host_arch=$(uname -m)
case "$host_os:$host_arch" in
  Linux:x86_64) native_target=x86_64-unknown-linux-gnu ;;
  Darwin:arm64 | Darwin:aarch64) native_target=aarch64-apple-darwin ;;
  *) die "C-Ray benchmark requires native x86-64 Linux or Apple-silicon macOS" ;;
esac
if [[ -z "$target" ]]; then
  target=$native_target
fi
[[ "$target" == "$native_target" ]] ||
  die "C-Ray benchmark target $target is not native host target $native_target"

if [[ -z "$warmups" ]]; then
  warmups=$(manifest_integer "${profile}_default_warmups")
fi
if [[ -z "$samples" ]]; then
  samples=$(manifest_integer "${profile}_default_samples")
fi
nonnegative_integer "--warmups" "$warmups"
positive_integer "--samples" "$samples"

for tool in awk bash cmp find grep mkdir mktemp mv openssl python3 rm sed \
  sort tar tr uname wc; do
  require_tool "$tool"
done

version=$(manifest_string version)
origin=$(manifest_string origin)
archive_name=$(manifest_string archive)
expected_archive_bytes=$(manifest_integer archive_bytes)
expected_archive_sha256=$(manifest_string archive_sha256)
expected_archive_sha3=$(manifest_string archive_sha3_256)
expected_source_bytes=$(manifest_integer source_bytes)
expected_source_sha256=$(manifest_string source_sha256)
scene_name=$(manifest_string "${profile}_scene")
expected_scene_bytes=$(manifest_integer "${profile}_scene_bytes")
expected_scene_sha256=$(manifest_string "${profile}_scene_sha256")
width=$(manifest_integer "${profile}_width")
height=$(manifest_integer "${profile}_height")
threads=$(manifest_integer "${profile}_threads")
rays_per_pixel=$(manifest_integer "${profile}_rays_per_pixel")

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-c-ray-$profile.XXXXXX")
else
  if [[ -e "$work_directory" && ! -d "$work_directory" ]]; then
    usage_error "C-Ray work path is not a directory: $work_directory"
  fi
  if [[ -d "$work_directory" ]] &&
    [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    usage_error "C-Ray work directory must be empty: $work_directory"
  fi
  mkdir -p "$work_directory"
fi
work_directory=$(absolute_directory "$work_directory")

cache_directory=${XDG_CACHE_HOME:-$HOME/.cache}/ccc/corpus/c-ray
if [[ -z "$source_archive" ]]; then
  mkdir -p "$cache_directory"
  source_archive="$cache_directory/$archive_name"
  download_archive "$origin" "$source_archive"
fi
source_archive=$(absolute_file "$source_archive")
verify_archive \
  "$source_archive" \
  "$expected_archive_bytes" \
  "$expected_archive_sha256" \
  "$expected_archive_sha3"

source_parent="$work_directory/source"
mkdir -p "$source_parent"
tar -xzf "$source_archive" -C "$source_parent"
source_directory="$source_parent/c-ray-$version"
source_file="$source_directory/$(manifest_string source)"
scene_file="$source_directory/$scene_name"
[[ -f "$source_file" && -f "$scene_file" ]] ||
  die "C-Ray source archive has an unexpected layout"
verify_file \
  "C-Ray source" \
  "$source_file" \
  "$expected_source_bytes" \
  "$expected_source_sha256"
verify_file \
  "C-Ray $scene_name scene" \
  "$scene_file" \
  "$expected_scene_bytes" \
  "$expected_scene_sha256"
grep -Fq 'Copyright (C) 2006 John Tsiombikas' "$source_file" &&
  grep -Fq 'GNU General Public License v2 or (at your option) later' "$source_file" ||
  die "C-Ray GPL-2.0-or-later source notice is missing"
sed -n '1,7p' "$source_file" >"$work_directory/upstream-license-notice.txt"

ccc=$(resolve_executable "$ccc")
resource_directory=$(absolute_directory "$resource_directory")
if [[ -z "$reference_cc" ]]; then
  case "$target" in
    x86_64-unknown-linux-gnu) reference_cc=gcc ;;
    aarch64-apple-darwin) reference_cc=/usr/bin/clang ;;
  esac
fi
reference_cc=$(resolve_executable "$reference_cc")

ccc_target_arguments=("--target=$target")
reference_target_arguments=()
case "$target" in
  x86_64-unknown-linux-gnu)
    section_size_tool=$(resolve_executable size)
    [[ -z "$sdk_root" ]] ||
      usage_error "--sdk-root is valid only for the Darwin arm64 profile"
    reported_reference_target=$(LC_ALL=C "$reference_cc" -dumpmachine)
    [[ "$reported_reference_target" =~ ^x86_64(-[[:alnum:]_.]+)?-linux-gnu ]] ||
      die "reference compiler target is $reported_reference_target rather than x86-64 Linux GNU"
    ;;
  aarch64-apple-darwin)
    require_tool xcrun
    section_size_tool=$(xcrun --find llvm-size) ||
      die "xcrun could not locate llvm-size"
    section_size_tool=$(resolve_executable "$section_size_tool")
    reference_version=$(LC_ALL=C "$reference_cc" --version)
    [[ "$(printf '%s\n' "$reference_version" | tr '[:upper:]' '[:lower:]')" == *clang* ]] ||
      die "Darwin C-Ray reference/link driver must be Clang"
    if [[ -z "$sdk_root" ]]; then
      require_tool xcrun
      sdk_root=$(xcrun --sdk macosx --show-sdk-path)
    fi
    sdk_root=$(absolute_directory "$sdk_root")
    ccc_target_arguments+=(
      "--sdk-root=$sdk_root"
      "-mmacosx-version-min=$deployment_target"
    )
    reference_target_arguments=(
      -target "arm64-apple-macos$deployment_target"
      -isysroot "$sdk_root"
      "-mmacosx-version-min=$deployment_target"
    )
    reported_reference_target=$(LC_ALL=C "$reference_cc" -dumpmachine)
    [[ "$reported_reference_target" =~ ^(aarch64|arm64)-apple-(darwin|macosx) ]] ||
      die "reference compiler target is $reported_reference_target rather than Darwin arm64"
    ;;
esac

export CCC_RESOURCE_DIR="$resource_directory"
compiler_identity_directory="$work_directory/compiler-identities"
tool_output_directory="$work_directory/tool-output"
timing_directory="$work_directory/timings"
stderr_directory="$work_directory/run-stderr"
build_directory="$work_directory/build"
mkdir -p \
  "$compiler_identity_directory" \
  "$tool_output_directory" \
  "$timing_directory" \
  "$stderr_directory" \
  "$build_directory"
commands="$work_directory/commands.txt"
timings="$work_directory/timings.tsv"
artifact_sizes="$work_directory/artifact-sizes.tsv"
output_hashes="$work_directory/output-sha256.tsv"
: >"$commands"
printf 'label\tobject_bytes\texecutable_bytes\n' >"$artifact_sizes"
printf 'label\tphase\titeration\tsha256\n' >"$output_hashes"

LC_ALL=C "$ccc" --version >"$compiler_identity_directory/ccc-version.txt" 2>&1
LC_ALL=C "$ccc" "${ccc_target_arguments[@]}" -dM -E -x c /dev/null \
  >"$compiler_identity_directory/ccc-macros.txt"
LC_ALL=C "$reference_cc" --version \
  >"$compiler_identity_directory/reference-version.txt" 2>&1
LC_ALL=C "$section_size_tool" --version \
  >"$compiler_identity_directory/section-size-version.txt" 2>&1
LC_ALL=C "$reference_cc" "${reference_target_arguments[@]}" -dM -E -x c /dev/null \
  >"$compiler_identity_directory/reference-macros.txt"
printf '%s\n' "$reported_reference_target" \
  >"$compiler_identity_directory/reference-target.txt"

for macros in \
  "$compiler_identity_directory/ccc-macros.txt" \
  "$compiler_identity_directory/reference-macros.txt"; do
  grep -Eq '^#define __BYTE_ORDER__[[:space:]]+__ORDER_LITTLE_ENDIAN__$' "$macros" &&
    grep -Eq '^#define __SIZEOF_POINTER__[[:space:]]+8$' "$macros" ||
    die "compiler does not expose the required little-endian LP64 target contract: $macros"
done
case "$target" in
  x86_64-unknown-linux-gnu)
    for macros in \
      "$compiler_identity_directory/ccc-macros.txt" \
      "$compiler_identity_directory/reference-macros.txt"; do
      grep -Eq '^#define __x86_64__[[:space:]]+1$' "$macros" ||
        die "compiler does not expose x86-64 target identity: $macros"
    done
    ;;
  aarch64-apple-darwin)
    for macros in \
      "$compiler_identity_directory/ccc-macros.txt" \
      "$compiler_identity_directory/reference-macros.txt"; do
      grep -Eq '^#define (__aarch64__|__arm64__)[[:space:]]+1$' "$macros" &&
        grep -Eq '^#define __APPLE__[[:space:]]+1$' "$macros" ||
        die "compiler does not expose Apple arm64 target identity: $macros"
    done
    ;;
esac

archive_sha256=$(hash_file sha256 "$source_archive")
archive_sha3=$(hash_file sha3-256 "$source_archive")
{
  printf 'bytes=%s\n' "$expected_archive_bytes"
  printf 'sha256=%s\n' "$archive_sha256"
  printf 'sha3_256=%s\n' "$archive_sha3"
} >"$work_directory/archive-hashes.txt"
{
  printf 'source_sha256=%s\n' "$(hash_file sha256 "$source_file")"
  printf 'scene_name=%s\n' "$scene_name"
  printf 'scene_sha256=%s\n' "$(hash_file sha256 "$scene_file")"
} >"$work_directory/source-hashes.txt"
{
  printf 'corpus=c-ray\n'
  printf 'version=%s\n' "$version"
  printf 'revision=%s\n' "$(manifest_string revision)"
  printf 'profile=%s\n' "$profile"
  printf 'target=%s\n' "$target"
  printf 'width=%s\n' "$width"
  printf 'height=%s\n' "$height"
  printf 'threads=%s\n' "$threads"
  printf 'rays_per_pixel=%s\n' "$rays_per_pixel"
  printf 'warmups=%s\n' "$warmups"
  printf 'samples=%s\n' "$samples"
  printf 'floating_point=strict-no-fast-math\n'
  printf 'byte_order=little-endian-verified\n'
  printf 'ccc=%s\n' "$ccc"
  printf 'reference_cc=%s\n' "$reference_cc"
  printf 'section_size_tool=%s\n' "$section_size_tool"
  if [[ -n "$sdk_root" ]]; then
    printf 'sdk_root=%s\n' "$sdk_root"
    printf 'deployment_target=%s\n' "$deployment_target"
  fi
} >"$work_directory/run-config.txt"

labels=()
executables=()
for optimization in -O0 -O2 -Oz; do
  optimization_name=$(printf '%s' "${optimization#-}" | tr '[:upper:]' '[:lower:]')
  label="ccc-$optimization_name"
  object="$build_directory/$label.o"
  executable="$build_directory/$label"
  labels+=("$label")
  executables+=("$executable")
  measure \
    compile "$label" 0 \
    "$tool_output_directory/$label-compile.stdout" \
    "$tool_output_directory/$label-compile.stderr" \
    "$ccc" \
    "${ccc_target_arguments[@]}" \
    -std=c11 "$optimization" -DLITTLE_ENDIAN=1 -pthread \
    -c "$source_file" -o "$object"
  measure \
    link "$label" 0 \
    "$tool_output_directory/$label-link.stdout" \
    "$tool_output_directory/$label-link.stderr" \
    "$reference_cc" \
    "${reference_target_arguments[@]}" \
    "$object" -o "$executable" -pthread -lm
  [[ -f "$object" && -x "$executable" ]] ||
    die "$label did not produce the expected object and executable"
  printf '%s\t%s\t%s\n' \
    "$label" \
    "$(wc -c <"$object" | tr -d '[:space:]')" \
    "$(wc -c <"$executable" | tr -d '[:space:]')" \
    >>"$artifact_sizes"
done

reference_label=reference-o2
reference_object="$build_directory/$reference_label.o"
reference_executable="$build_directory/$reference_label"
labels+=("$reference_label")
executables+=("$reference_executable")
measure \
  compile "$reference_label" 0 \
  "$tool_output_directory/$reference_label-compile.stdout" \
  "$tool_output_directory/$reference_label-compile.stderr" \
  "$reference_cc" \
  "${reference_target_arguments[@]}" \
  -std=c11 -O2 -fno-fast-math -ffp-contract=off \
  -DLITTLE_ENDIAN=1 -pthread \
  -c "$source_file" -o "$reference_object"
measure \
  link "$reference_label" 0 \
  "$tool_output_directory/$reference_label-link.stdout" \
  "$tool_output_directory/$reference_label-link.stderr" \
  "$reference_cc" \
  "${reference_target_arguments[@]}" \
  "$reference_object" -o "$reference_executable" -pthread -lm
[[ -f "$reference_object" && -x "$reference_executable" ]] ||
  die "$reference_label did not produce the expected object and executable"
printf '%s\t%s\t%s\n' \
  "$reference_label" \
  "$(wc -c <"$reference_object" | tr -d '[:space:]')" \
  "$(wc -c <"$reference_executable" | tr -d '[:space:]')" \
  >>"$artifact_sizes"

object_section_arguments=()
for label in "${labels[@]}"; do
  object_section_arguments+=(
    --artifact "$label" "$build_directory/$label.o"
  )
done
"$collect_object_sections" \
  --size-tool "$section_size_tool" \
  --sections-output "$work_directory/object-sections.tsv" \
  --totals-output "$work_directory/object-section-totals.tsv" \
  --raw-output "$work_directory/object-sections.txt" \
  "${object_section_arguments[@]}"

: >"$work_directory/object-size.txt"
: >"$work_directory/executable-size.txt"
for ((index = 0; index < ${#labels[@]}; index++)); do
  label=${labels[$index]}
  executable=${executables[$index]}
  {
    printf '%s\n' "== $label =="
    "$section_size_tool" "$build_directory/$label.o"
  } >>"$work_directory/object-size.txt"
  {
    printf '%s\n' "== $label =="
    "$section_size_tool" "$executable"
  } >>"$work_directory/executable-size.txt"
done

reference_image="$work_directory/reference.ppm"
measure \
  render-check "$reference_label" 0 \
  "$tool_output_directory/$reference_label-check.stdout" \
  "$stderr_directory/$reference_label-check.stderr" \
  "$reference_executable" \
  -t "$threads" -r "$rays_per_pixel" -s "${width}x${height}" \
  -i "$scene_file" -o "$reference_image"
reference_hash=$(
  "$validate_ppm" \
    --path "$reference_image" \
    --width "$width" \
    --height "$height"
)
printf '%s\tcheck\t0\t%s\n' "$reference_label" "$reference_hash" \
  >>"$output_hashes"

for ((index = 0; index < 3; index++)); do
  label=${labels[$index]}
  executable=${executables[$index]}
  candidate="$build_directory/$label-check.ppm"
  measure \
    render-check "$label" 0 \
    "$tool_output_directory/$label-check.stdout" \
    "$stderr_directory/$label-check.stderr" \
    "$executable" \
    -t "$threads" -r "$rays_per_pixel" -s "${width}x${height}" \
    -i "$scene_file" -o "$candidate"
  candidate_hash=$(
    "$validate_ppm" \
      --path "$candidate" \
      --width "$width" \
      --height "$height" \
      --compare "$reference_image"
  )
  printf '%s\tcheck\t0\t%s\n' "$label" "$candidate_hash" \
    >>"$output_hashes"
  rm -f -- "$candidate"
done

for ((index = 0; index < ${#labels[@]}; index++)); do
  label=${labels[$index]}
  executable=${executables[$index]}
  candidate="$build_directory/$label-sample.ppm"
  for ((iteration = 1; iteration <= warmups; iteration++)); do
    measure \
      render-warmup "$label" "$iteration" \
      "$tool_output_directory/$label-warmup-$iteration.stdout" \
      "$stderr_directory/$label-warmup-$iteration.stderr" \
      "$executable" \
      -t "$threads" -r "$rays_per_pixel" -s "${width}x${height}" \
      -i "$scene_file" -o "$candidate"
    candidate_hash=$(
      "$validate_ppm" \
        --path "$candidate" \
        --width "$width" \
        --height "$height" \
        --compare "$reference_image"
    )
    printf '%s\twarmup\t%s\t%s\n' \
      "$label" "$iteration" "$candidate_hash" >>"$output_hashes"
  done
  for ((iteration = 1; iteration <= samples; iteration++)); do
    measure \
      render-sample "$label" "$iteration" \
      "$tool_output_directory/$label-sample-$iteration.stdout" \
      "$stderr_directory/$label-sample-$iteration.stderr" \
      "$executable" \
      -t "$threads" -r "$rays_per_pixel" -s "${width}x${height}" \
      -i "$scene_file" -o "$candidate"
    candidate_hash=$(
      "$validate_ppm" \
        --path "$candidate" \
        --width "$width" \
        --height "$height" \
        --compare "$reference_image"
    )
    printf '%s\tsample\t%s\t%s\n' \
      "$label" "$iteration" "$candidate_hash" >>"$output_hashes"
  done
  rm -f -- "$candidate"
done

"$summarize" \
  --timings "$timings" \
  --artifacts "$artifact_sizes" \
  --object-sections "$work_directory/object-section-totals.tsv" \
  --hashes "$output_hashes" \
  --output "$work_directory/summary.tsv"

printf 'C-Ray %s %s artifacts: %s\n' "$version" "$profile" "$work_directory"
