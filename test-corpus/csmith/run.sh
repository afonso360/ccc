#!/usr/bin/env bash

set -euo pipefail

script_directory=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repository=$(CDPATH= cd -- "$script_directory/../.." && pwd -P)
source "$script_directory/profile.sh"
source "$repository/test-corpus/adapter-environment.sh"

usage() {
  cat >&2 <<EOF
usage: $0 [OPTIONS]

Run reproducible Csmith programs through CCC and a GCC/Clang reference matrix.

  --cases COUNT                 admissible differential cases (default: ${default_cases})
  --start-seed SEED             first attempted seed (default: ${default_start_seed})
  --max-attempts COUNT          seed-attempt limit (default: ${default_maximum_attempt_multiplier} times --cases)
  --build-jobs COUNT            jobs used to build Csmith (default: ${default_build_jobs})
  --generator-timeout SECONDS   per-program generation timeout (default: ${default_generator_timeout})
  --compile-timeout SECONDS     per-compiler timeout (default: ${default_compile_timeout})
  --run-timeout SECONDS         per-executable timeout (default: ${default_execution_timeout})
  --work-dir PATH               empty artifact directory (default: a retained temp directory)
  --archive PATH                pinned Csmith source archive; disables download
  --csmith PATH                 developer-supplied Csmith ${csmith_version} executable
  --csmith-runtime PATH         runtime include directory paired with --csmith
  --allow-unverified-csmith     permit the developer-supplied generator override
  --ccc PATH                    CCC executable (default: target/debug/ccc)
  --resource-dir PATH           CCC resource directory (default: resource-dir)
  --gcc PATH                    native reference GCC (default: gcc)
  --clang PATH                  native reference Clang (default: clang)
  --objcopy PATH                object copier used by CCC (default: objcopy)
  --cxx PATH                    C++ compiler used to build Csmith (default: g++)
  -h, --help                    show this help
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
  command -v "$1" >/dev/null 2>&1 || die "required tool is not available: $1"
}

absolute_directory() {
  [[ -d "$1" ]] || die "directory does not exist: $1"
  (CDPATH= cd -- "$1" && pwd -P)
}

absolute_file() {
  [[ -f "$1" ]] || die "file does not exist: $1"
  printf '%s/%s\n' \
    "$(CDPATH= cd -- "$(dirname -- "$1")" && pwd -P)" \
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

positive_integer() {
  [[ "$2" =~ ^[1-9][0-9]*$ ]] || usage_error "$1 must be a positive integer: $2"
}

nonnegative_integer() {
  [[ "$2" =~ ^(0|[1-9][0-9]*)$ ]] || usage_error "$1 must be a nonnegative integer: $2"
}

validate_path_argument() {
  local label=$1
  local value=$2
  [[ "$value" != *$'\n'* && "$value" != *$'\r'* && "$value" != *$'\t'* ]] ||
    usage_error "$label cannot contain tabs or newlines"
}

verify_native_target() {
  local label=$1
  local target=$2
  [[ "$target" =~ ^x86_64(-[[:alnum:]_.]+)?-linux-gnu$ ]] ||
    die "$label target is $target rather than x86-64 Linux GNU"
}

verify_lp64_x86_64_macros() {
  local label=$1
  local macros=$2
  grep -Eq '^#define __x86_64__[[:space:]]+1$' "$macros" &&
    grep -Eq '^#define __SIZEOF_POINTER__[[:space:]]+8$' "$macros" &&
    grep -Eq '^#define __SIZEOF_LONG__[[:space:]]+8$' "$macros" &&
    grep -Eq '^#define __LP64__[[:space:]]+1$' "$macros" &&
    grep -Eq '^#define __BYTE_ORDER__[[:space:]]+__ORDER_LITTLE_ENDIAN__$' "$macros" ||
    die "$label does not expose the required little-endian x86-64 LP64 ABI"
}

record_command() {
  local destination=$1
  shift
  printf 'LC_ALL=C' >>"$destination"
  printf ' %q' "$@" >>"$destination"
  printf '\n' >>"$destination"
}

run_timed() {
  local seconds=$1
  local standard_output=$2
  local standard_error=$3
  local command_log=$4
  local status_file=$5
  local status
  shift 5

  record_command "$command_log" timeout --kill-after=2s "${seconds}s" "$@"
  if timeout --kill-after=2s "${seconds}s" "$@" \
    >"$standard_output" 2>"$standard_error"; then
    status=0
  else
    status=$?
  fi
  printf '%s\n' "$status" >"$status_file"
}

write_result() {
  local directory=$1
  local outcome=$2
  local detail=$3
  local admissible=$4
  local completed=$5
  local failure=$6
  local partial_result="$directory/result.txt.part.$$"
  {
    printf 'outcome=%s\n' "$outcome"
    printf 'detail=%s\n' "$detail"
    printf 'admissible=%s\n' "$admissible"
    printf 'completed=%s\n' "$completed"
    printf 'failure=%s\n' "$failure"
  } >"$partial_result"
  mv -f -- "$partial_result" "$directory/result.txt"
  printf '%s: %s\n' "$outcome" "$detail"
}

valid_result() {
  local result=$1
  [[ -f "$result" ]] &&
    [[ "$(wc -l <"$result" | tr -d '[:space:]')" == 5 ]] &&
    [[ "$(grep -Ec '^outcome=[a-z][a-z-]*$' "$result")" == 1 ]] &&
    [[ "$(grep -Ec '^detail=.+$' "$result")" == 1 ]] &&
    [[ "$(grep -Ec '^admissible=[01]$' "$result")" == 1 ]] &&
    [[ "$(grep -Ec '^completed=[01]$' "$result")" == 1 ]] &&
    [[ "$(grep -Ec '^failure=[01]$' "$result")" == 1 ]]
}

status_is_zero() {
  [[ -f "$1" && "$(tr -d '[:space:]' <"$1")" == 0 ]]
}

status_is_rejection() {
  [[ -f "$1" && "$(tr -d '[:space:]' <"$1")" == 1 ]]
}

status_is_timeout() {
  local status
  [[ -f "$1" ]] || return 1
  status=$(tr -d '[:space:]' <"$1")
  [[ "$status" == 124 ]]
}

syntax_diagnostics_show_crash() {
  grep -Eiq \
    'internal compiler error|please submit (a |this )?bug report|stack dump|segmentation fault' \
    "$@"
}

run_case() {
  local seed=$1
  local directory=$2
  local source_file="$directory/program.c"
  local command_log="$directory/commands.txt"
  local generator_status="$directory/generator.status"
  local label compiler optimization executable csmith_version_pattern
  local compile_status run_status
  local index baseline_stdout baseline_stderr timeout_count nonzero_count
  local gcc_syntax_status="$directory/gcc.syntax.status"
  local clang_syntax_status="$directory/clang.syntax.status"
  local ccc_object="$directory/ccc.o"
  local ccc_executable="$directory/ccc.exe"

  : >"$command_log"
  run_timed \
    "$generator_timeout" \
    "$source_file" \
    "$directory/generator.stderr" \
    "$command_log" \
    "$generator_status" \
    "$csmith" "${generator_options[@]}" --seed "$seed"

  if ! status_is_zero "$generator_status"; then
    write_result "$directory" generator-failure \
      "Csmith failed or timed out for seed $seed" 0 0 1
    return
  fi
  csmith_version_pattern=${csmith_version//./\\.}
  if ! grep -Eq \
    "Generator:[[:space:]]+csmith[[:space:]]+${csmith_version_pattern}[[:space:]]*$" \
    "$source_file" ||
    ! grep -Eq "Seed:[[:space:]]+${seed}[[:space:]]*$" "$source_file"; then
    write_result "$directory" generator-provenance-failure \
      "generated source does not record the pinned version and seed $seed" 0 0 1
    return
  fi

  run_timed \
    "$compile_timeout" \
    "$directory/gcc.syntax.stdout" \
    "$directory/gcc.syntax.stderr" \
    "$command_log" \
    "$gcc_syntax_status" \
    "$reference_gcc" -std=c11 -pedantic-errors -fsyntax-only \
    -I "$csmith_runtime" "$source_file"
  run_timed \
    "$compile_timeout" \
    "$directory/clang.syntax.stdout" \
    "$directory/clang.syntax.stderr" \
    "$command_log" \
    "$clang_syntax_status" \
    "$reference_clang" -std=c11 -pedantic-errors -fsyntax-only \
    -I "$csmith_runtime" "$source_file"

  if status_is_timeout "$gcc_syntax_status" ||
    status_is_timeout "$clang_syntax_status"; then
    write_result "$directory" reference-syntax-failure \
      "strict C11 validation timed out for seed $seed" 0 0 1
    return
  fi
  if syntax_diagnostics_show_crash \
    "$directory/gcc.syntax.stderr" "$directory/clang.syntax.stderr"; then
    write_result "$directory" reference-syntax-failure \
      "a strict C11 validator reported an internal failure for seed $seed" 0 0 1
    return
  fi
  if status_is_rejection "$gcc_syntax_status" &&
    status_is_rejection "$clang_syntax_status"; then
    write_result "$directory" inadmissible \
      "GCC and Clang both reject seed $seed as strict C11" 0 0 0
    return
  fi
  if { status_is_zero "$gcc_syntax_status" &&
    status_is_rejection "$clang_syntax_status"; } ||
    { status_is_rejection "$gcc_syntax_status" &&
      status_is_zero "$clang_syntax_status"; }; then
    write_result "$directory" reference-syntax-disagreement \
      "only one reference compiler accepts seed $seed as strict C11" 0 0 1
    return
  fi
  if ! status_is_zero "$gcc_syntax_status" ||
    ! status_is_zero "$clang_syntax_status"; then
    write_result "$directory" reference-syntax-failure \
      "a strict C11 validator exited abnormally for seed $seed" 0 0 1
    return
  fi

  for ((index = 0; index < ${#reference_labels[@]}; index++)); do
    label=${reference_labels[$index]}
    compiler=${reference_compilers[$index]}
    optimization=${reference_options[$index]}
    executable="$directory/reference-${label}.exe"
    compile_status="$directory/reference-${label}.compile.status"
    run_status="$directory/reference-${label}.run.status"

    run_timed \
      "$compile_timeout" \
      "$directory/reference-${label}.compile.stdout" \
      "$directory/reference-${label}.compile.stderr" \
      "$command_log" \
      "$compile_status" \
      "$compiler" -std=c11 "$optimization" -fno-pie -no-pie \
      -I "$csmith_runtime" "$source_file" -o "$executable" -lm
    if ! status_is_zero "$compile_status"; then
      write_result "$directory" reference-compile-failure \
        "$label failed or timed out while compiling seed $seed" 1 0 1
      return
    fi
  done

  for ((index = 0; index < ${#reference_labels[@]}; index++)); do
    label=${reference_labels[$index]}
    executable="$directory/reference-${label}.exe"
    run_status="$directory/reference-${label}.run.status"
    run_timed \
      "$execution_timeout" \
      "$directory/reference-${label}.run.stdout" \
      "$directory/reference-${label}.run.stderr" \
      "$command_log" \
      "$run_status" \
      "$executable"
  done

  timeout_count=0
  nonzero_count=0
  for ((index = 0; index < ${#reference_labels[@]}; index++)); do
    label=${reference_labels[$index]}
    run_status="$directory/reference-${label}.run.status"
    if status_is_timeout "$run_status"; then
      ((timeout_count += 1))
    elif ! status_is_zero "$run_status"; then
      ((nonzero_count += 1))
    fi
  done
  if ((timeout_count == ${#reference_labels[@]})); then
    write_result "$directory" inconclusive-timeout \
      "all references timed out for seed $seed" 1 0 0
    return
  fi
  if ((timeout_count > 0 || nonzero_count > 0)); then
    write_result "$directory" reference-execution-failure \
      "reference statuses do not establish an executable oracle for seed $seed" 1 0 1
    return
  fi

  baseline_stdout="$directory/reference-${reference_labels[0]}.run.stdout"
  baseline_stderr="$directory/reference-${reference_labels[0]}.run.stderr"
  if [[ -s "$baseline_stderr" ]] ||
    [[ "$(wc -l <"$baseline_stdout" | tr -d '[:space:]')" != 1 ]] ||
    ! grep -Eq '^checksum = [0-9A-F]+$' "$baseline_stdout"; then
    write_result "$directory" reference-invalid-output \
      "reference output is not one checksum line with empty stderr for seed $seed" 1 0 1
    return
  fi
  for ((index = 1; index < ${#reference_labels[@]}; index++)); do
    label=${reference_labels[$index]}
    if ! cmp -s "$baseline_stdout" "$directory/reference-${label}.run.stdout" ||
      ! cmp -s "$baseline_stderr" "$directory/reference-${label}.run.stderr"; then
      write_result "$directory" reference-disagreement \
        "GCC and Clang references disagree for seed $seed" 1 0 1
      return
    fi
  done

  run_timed \
    "$compile_timeout" \
    "$directory/ccc.compile.stdout" \
    "$directory/ccc.compile.stderr" \
    "$command_log" \
    "$directory/ccc.compile.status" \
    "$ccc" -resource-dir "$ccc_resource_dir" -std=c11 -c \
    -I "$csmith_runtime" "$source_file" -o "$ccc_object"
  if ! status_is_zero "$directory/ccc.compile.status"; then
    write_result "$directory" ccc-compile-failure \
      "CCC failed or timed out while compiling seed $seed" 1 1 1
    return
  fi

  run_timed \
    "$compile_timeout" \
    "$directory/ccc.link.stdout" \
    "$directory/ccc.link.stderr" \
    "$command_log" \
    "$directory/ccc.link.status" \
    "$reference_gcc" -fno-pie -no-pie "$ccc_object" -o "$ccc_executable" -lm
  if ! status_is_zero "$directory/ccc.link.status"; then
    write_result "$directory" ccc-link-failure \
      "GCC failed or timed out while linking CCC's object for seed $seed" 1 1 1
    return
  fi

  run_timed \
    "$execution_timeout" \
    "$directory/ccc.run.stdout" \
    "$directory/ccc.run.stderr" \
    "$command_log" \
    "$directory/ccc.run.status" \
    "$ccc_executable"
  if ! status_is_zero "$directory/ccc.run.status"; then
    write_result "$directory" ccc-execution-failure \
      "CCC output failed or timed out while executing seed $seed" 1 1 1
    return
  fi

  if ! cmp -s "$baseline_stdout" "$directory/ccc.run.stdout" ||
    ! cmp -s "$baseline_stderr" "$directory/ccc.run.stderr"; then
    write_result "$directory" output-mismatch \
      "CCC output differs from the reference consensus for seed $seed" 1 1 1
    return
  fi

  write_result "$directory" pass "seed $seed" 1 1 0
}

verify_archive() {
  archive_matches "$1" || die "Csmith archive does not match the source pin"
}

archive_matches() {
  local path=$1
  local actual_bytes actual_sha256 actual_sha3

  [[ -f "$path" ]] || return 1
  actual_bytes=$(wc -c <"$path" | tr -d '[:space:]')
  actual_sha256=$(openssl dgst -sha256 "$path" | awk '{print $NF}')
  actual_sha3=$(openssl dgst -sha3-256 "$path" | awk '{print $NF}')
  [[ "$actual_bytes" == "$csmith_archive_bytes" ]] &&
    [[ "$actual_sha256" == "$csmith_archive_sha256" ]] &&
    [[ "$actual_sha3" == "$csmith_archive_sha3_256" ]]
}

prepare_csmith() {
  local archive_path=$1
  local source_directory="$work_directory/csmith-source"
  local build_directory="$work_directory/csmith-build"
  local install_directory="$work_directory/csmith-install"
  local partial_archive cache_directory cache_root
  local csmith_cxx_target csmith_cxx_version

  for tool in cmake m4 tar; do
    require_tool "$tool"
  done
  csmith_cxx=$(resolve_executable "$csmith_cxx")
  csmith_cxx_target=$(LC_ALL=C "$csmith_cxx" -dumpmachine) ||
    die "Csmith C++ compiler did not report its target"
  csmith_cxx_version=$(LC_ALL=C "$csmith_cxx" --version) ||
    die "Csmith C++ compiler did not report its identity"
  verify_native_target "Csmith C++ compiler" "$csmith_cxx_target"
  LC_ALL=C "$csmith_cxx" -dM -E -x c++ /dev/null \
    >"$work_directory/tool-identities/csmith-cxx-macros.txt" ||
    die "Csmith C++ compiler did not report predefined macros"
  verify_lp64_x86_64_macros "Csmith C++ compiler" \
    "$work_directory/tool-identities/csmith-cxx-macros.txt"
  {
    printf 'executable=%s\n' "$csmith_cxx"
    printf 'target=%s\n' "$csmith_cxx_target"
    openssl dgst -sha256 "$csmith_cxx"
    printf '%s\n' '--version:' "$csmith_cxx_version"
  } >"$work_directory/tool-identities/csmith-cxx.txt"

  if [[ -z "$archive_path" ]]; then
    require_tool curl
    if [[ -n "${XDG_CACHE_HOME:-}" ]]; then
      cache_root=$XDG_CACHE_HOME
    elif [[ -n "${HOME:-}" ]]; then
      cache_root=$HOME/.cache
    else
      die "HOME or XDG_CACHE_HOME is required to cache the Csmith archive"
    fi
    cache_directory=$cache_root/ccc/corpus/csmith
    mkdir -p -- "$cache_directory"
    archive_path="$cache_directory/$csmith_archive_name"
    if [[ ! -f "$archive_path" ]] || ! archive_matches "$archive_path"; then
      partial_archive="$archive_path.part.$$"
      if ! curl --fail --silent --show-error --location --retry 3 \
        --connect-timeout 30 --max-time 300 \
        --output "$partial_archive" "$csmith_archive_origin"; then
        rm -f -- "$partial_archive"
        die "failed to download pinned Csmith source"
      fi
      if ! archive_matches "$partial_archive"; then
        rm -f -- "$partial_archive"
        die "downloaded Csmith archive does not match the source pin"
      fi
      mv -f -- "$partial_archive" "$archive_path"
    fi
  fi
  archive_path=$(absolute_file "$archive_path")
  verify_archive "$archive_path"

  mkdir -p -- "$source_directory" "$build_directory" "$install_directory"
  tar -xzf "$archive_path" -C "$source_directory" --strip-components=1
  grep -Eq 'set\(csmith_PACKAGE_VERSION[[:space:]]+"2\.4\.0"\)' \
    "$source_directory/CMakeLists.txt" || die "Csmith source version does not match the pin"
  grep -Fq 'Redistribution and use in source and binary forms' \
    "$source_directory/COPYING" || die "Csmith source license is missing"

  cmake \
    -S "$source_directory" \
    -B "$build_directory" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$install_directory" \
    -DCMAKE_C_COMPILER="$reference_gcc" \
    -DCMAKE_CXX_COMPILER="$csmith_cxx" \
    >"$work_directory/csmith-configure.log" 2>&1
  cmake --build "$build_directory" --parallel "$build_jobs" \
    >"$work_directory/csmith-build.log" 2>&1
  cmake --install "$build_directory" --prefix "$install_directory" \
    >"$work_directory/csmith-install.log" 2>&1

  csmith="$install_directory/bin/csmith"
  csmith_runtime="$install_directory/include"
}

cases=${CSMITH_CASES:-$default_cases}
start_seed=${CSMITH_START_SEED:-$default_start_seed}
max_attempts=${CSMITH_MAX_ATTEMPTS:-}
build_jobs=${CSMITH_BUILD_JOBS:-$default_build_jobs}
generator_timeout=${CSMITH_GENERATOR_TIMEOUT:-$default_generator_timeout}
compile_timeout=${CSMITH_COMPILE_TIMEOUT:-$default_compile_timeout}
execution_timeout=${CSMITH_RUN_TIMEOUT:-$default_execution_timeout}
work_directory=${CSMITH_WORK_DIR:-}
archive=${CSMITH_ARCHIVE:-}
csmith=${CSMITH:-}
csmith_runtime=${CSMITH_RUNTIME:-}
ccc=${CCC:-$repository/target/debug/ccc}
ccc_resource_dir=${CCC_RESOURCE_DIR:-$repository/resource-dir}
reference_gcc=${CSMITH_GCC:-gcc}
reference_clang=${CSMITH_CLANG:-clang}
object_copier=${CSMITH_OBJCOPY:-objcopy}
csmith_cxx=${CSMITH_CXX:-g++}
allow_unverified_csmith=0

while (($#)); do
  case "$1" in
    --cases|--start-seed|--max-attempts|--build-jobs|--generator-timeout|\
      --compile-timeout|--run-timeout|--work-dir|--archive|--csmith|\
      --csmith-runtime|--ccc|--resource-dir|--gcc|--clang|--objcopy|--cxx)
      (($# >= 2)) || usage_error "missing value for $1"
      option=$1
      value=$2
      case "$option" in
        --cases) cases=$value ;;
        --start-seed) start_seed=$value ;;
        --max-attempts) max_attempts=$value ;;
        --build-jobs) build_jobs=$value ;;
        --generator-timeout) generator_timeout=$value ;;
        --compile-timeout) compile_timeout=$value ;;
        --run-timeout) execution_timeout=$value ;;
        --work-dir) work_directory=$value ;;
        --archive) archive=$value ;;
        --csmith) csmith=$value ;;
        --csmith-runtime) csmith_runtime=$value ;;
        --ccc) ccc=$value ;;
        --resource-dir) ccc_resource_dir=$value ;;
        --gcc) reference_gcc=$value ;;
        --clang) reference_clang=$value ;;
        --objcopy) object_copier=$value ;;
        --cxx) csmith_cxx=$value ;;
      esac
      shift 2
      ;;
    --allow-unverified-csmith)
      allow_unverified_csmith=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage_error "unsupported option: $1"
      ;;
  esac
done

positive_integer "case count" "$cases"
((${#cases} <= 6)) || usage_error "case count exceeds the safety limit of 100000: $cases"
((cases <= 100000)) || usage_error "case count exceeds the safety limit of 100000: $cases"
if [[ -z "$max_attempts" ]]; then
  max_attempts=$((cases * default_maximum_attempt_multiplier))
fi
positive_integer "maximum attempt count" "$max_attempts"
positive_integer "Csmith build job count" "$build_jobs"
positive_integer "generator timeout" "$generator_timeout"
positive_integer "compile timeout" "$compile_timeout"
positive_integer "execution timeout" "$execution_timeout"
nonnegative_integer "start seed" "$start_seed"
((${#max_attempts} <= 7)) ||
  usage_error "maximum attempt count exceeds the safety limit of 1000000: $max_attempts"
((max_attempts <= 1000000)) ||
  usage_error "maximum attempt count exceeds the safety limit of 1000000: $max_attempts"
((max_attempts >= cases)) ||
  usage_error "maximum attempt count must be at least the requested case count"
((${#build_jobs} <= 3)) || usage_error "Csmith build job count exceeds 256: $build_jobs"
((build_jobs <= 256)) || usage_error "Csmith build job count exceeds 256: $build_jobs"
for timeout_value in "$generator_timeout" "$compile_timeout" "$execution_timeout"; do
  ((${#timeout_value} <= 5)) || usage_error "timeout exceeds 86400 seconds: $timeout_value"
  ((timeout_value <= 86400)) || usage_error "timeout exceeds 86400 seconds: $timeout_value"
done
((${#start_seed} <= 10)) || usage_error "start seed exceeds 4294967295: $start_seed"
((start_seed <= 4294967295)) || usage_error "start seed exceeds 4294967295: $start_seed"
last_possible_seed=$((start_seed + max_attempts - 1))
((last_possible_seed <= 4294967295)) || usage_error "attempted seed range exceeds 4294967295"

for path_entry in \
  "work directory:$work_directory" \
  "archive:$archive" \
  "Csmith executable:$csmith" \
  "Csmith runtime:$csmith_runtime" \
  "CCC executable:$ccc" \
  "CCC resource directory:$ccc_resource_dir" \
  "GCC executable:$reference_gcc" \
  "Clang executable:$reference_clang" \
  "object copier:$object_copier" \
  "C++ executable:$csmith_cxx"; do
  validate_path_argument "${path_entry%%:*}" "${path_entry#*:}"
done

if [[ -n "$csmith" || -n "$csmith_runtime" ]]; then
  [[ -n "$csmith" && -n "$csmith_runtime" ]] ||
    usage_error "--csmith and --csmith-runtime must be provided together"
  [[ -z "$archive" ]] ||
    usage_error "--archive cannot be combined with --csmith and --csmith-runtime"
  ((allow_unverified_csmith == 1)) ||
    usage_error "developer-supplied Csmith requires --allow-unverified-csmith"
elif ((allow_unverified_csmith == 1)); then
  usage_error "--allow-unverified-csmith requires --csmith and --csmith-runtime"
fi

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] ||
  die "Csmith differential execution requires x86-64 Linux"

for tool in awk basename cat cmp cp dirname env find grep mkdir mktemp mv openssl rm \
  sed sort timeout tr uname wc; do
  require_tool "$tool"
done

manifest_contents=$(cat "$script_directory/manifest.toml")
expected_manifest=$(write_csmith_manifest)
[[ "$manifest_contents" == "$expected_manifest" ]] ||
  die "Csmith manifest does not match profile.sh"

export LC_ALL=C
unset CFLAGS CPPFLAGS CXXFLAGS LDFLAGS LIBS
unset CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH
unset GCC_EXEC_PREFIX COMPILER_PATH LIBRARY_PATH
unset DESTDIR CMAKE_INSTALL_MODE CMAKE_GENERATOR CMAKE_GENERATOR_PLATFORM
unset CMAKE_GENERATOR_TOOLSET CMAKE_GENERATOR_INSTANCE CMAKE_BUILD_PARALLEL_LEVEL
unset CMAKE_INSTALL_PARALLEL_LEVEL CMAKE_PREFIX_PATH CMAKE_TOOLCHAIN_FILE
unset CMAKE_C_COMPILER_LAUNCHER CMAKE_CXX_COMPILER_LAUNCHER
unset CMAKE_C_LINKER_LAUNCHER CMAKE_CXX_LINKER_LAUNCHER
unset CMAKE_PROJECT_INCLUDE CMAKE_PROJECT_INCLUDE_BEFORE
unset CMAKE_PROJECT_TOP_LEVEL_INCLUDES CMAKE_USER_MAKE_RULES_OVERRIDE
unset CMAKE_USER_MAKE_RULES_OVERRIDE_C CMAKE_USER_MAKE_RULES_OVERRIDE_CXX
unset OBJCOPY
unset LD_PRELOAD LD_LIBRARY_PATH
clear_ambient_make_injection

reference_gcc=$(resolve_executable "$reference_gcc")
reference_clang=$(resolve_executable "$reference_clang")
object_copier=$(resolve_executable "$object_copier")
ccc=$(resolve_executable "$ccc")
ccc_resource_dir=$(absolute_directory "$ccc_resource_dir")
[[ "$reference_gcc" != *[[:space:]]* ]] ||
  die "GCC executable path cannot contain whitespace because CCC_CC is a command entry"
[[ "$object_copier" != *[[:space:]]* ]] ||
  die "objcopy path cannot contain whitespace because CCC_OBJCOPY is a command entry"
export CCC_CC="$reference_gcc"
export CCC_OBJCOPY="$object_copier"

if [[ -z "$work_directory" ]]; then
  work_directory=$(mktemp -d "${TMPDIR:-/tmp}/ccc-csmith.XXXXXX")
else
  if [[ -e "$work_directory" ]] && [[ ! -d "$work_directory" ]]; then
    usage_error "work path is not a directory: $work_directory"
  fi
  mkdir -p -- "$work_directory"
  work_directory=$(absolute_directory "$work_directory")
  if [[ -n "$(find "$work_directory" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    usage_error "work directory must be empty: $work_directory"
  fi
fi
work_directory=$(absolute_directory "$work_directory")
mkdir -p -- "$work_directory/cases" "$work_directory/tool-identities"
echo "Csmith artifacts: $work_directory"

record_native_gcc_driver \
  Csmith "$reference_gcc" \
  "$work_directory/tool-identities/gcc.txt" \
  "$work_directory/tool-identities/gcc-macros.txt"
openssl dgst -sha256 "$reference_gcc" \
  >>"$work_directory/tool-identities/gcc.txt"
gcc_target=$(sed -n 's/^target=//p' "$work_directory/tool-identities/gcc.txt")
verify_native_target "Csmith GCC reference" "$gcc_target"
verify_lp64_x86_64_macros "Csmith GCC reference" \
  "$work_directory/tool-identities/gcc-macros.txt"

clang_target=$(LC_ALL=C "$reference_clang" --print-target-triple) ||
  die "Csmith Clang reference did not report its target"
clang_version=$(LC_ALL=C "$reference_clang" --version) ||
  die "Csmith Clang reference did not report its identity"
LC_ALL=C "$reference_clang" -dM -E -x c /dev/null \
  >"$work_directory/tool-identities/clang-macros.txt" ||
  die "Csmith Clang reference did not report predefined macros"
[[ "$(printf '%s\n' "$clang_version" | tr '[:upper:]' '[:lower:]')" == *clang* ]] ||
  die "Csmith Clang reference is not Clang"
verify_native_target "Csmith Clang reference" "$clang_target"
grep -Eq '^#define __clang__[[:space:]]+1$' \
  "$work_directory/tool-identities/clang-macros.txt" ||
  die "Csmith Clang reference does not expose Clang identity macros"
verify_lp64_x86_64_macros "Csmith Clang reference" \
  "$work_directory/tool-identities/clang-macros.txt"
{
  printf 'executable=%s\n' "$reference_clang"
  printf 'target=%s\n' "$clang_target"
  openssl dgst -sha256 "$reference_clang"
  printf '%s\n' '--version:' "$clang_version"
} >"$work_directory/tool-identities/clang.txt"

object_copier_version=$(LC_ALL=C "$object_copier" --version) ||
  die "CCC object copier did not report its identity"
[[ "$(printf '%s\n' "$object_copier_version" |
  tr '[:upper:]' '[:lower:]')" == *objcopy* ]] ||
  die "CCC object copier does not identify itself as objcopy"
{
  printf 'executable=%s\n' "$object_copier"
  openssl dgst -sha256 "$object_copier"
  printf '%s\n' '--version:' "$object_copier_version"
} >"$work_directory/tool-identities/objcopy.txt"

if [[ -z "$csmith" ]]; then
  prepare_csmith "$archive"
else
  csmith=$(resolve_executable "$csmith")
  csmith_runtime=$(absolute_directory "$csmith_runtime")
fi
csmith=$(resolve_executable "$csmith")
csmith_runtime=$(absolute_directory "$csmith_runtime")
[[ -f "$csmith_runtime/csmith.h" ]] ||
  die "Csmith runtime header is missing: $csmith_runtime/csmith.h"

runtime_probe="$work_directory/tool-identities/csmith-runtime-probe.c"
runtime_probe_commands="$work_directory/tool-identities/csmith-runtime-probe.commands.txt"
printf '#include "csmith.h"\nint main(void) { return 0; }\n' >"$runtime_probe"
: >"$runtime_probe_commands"
for compiler_entry in "gcc:$reference_gcc" "clang:$reference_clang"; do
  compiler_label=${compiler_entry%%:*}
  compiler_path=${compiler_entry#*:}
  run_timed \
    "$compile_timeout" \
    "$work_directory/tool-identities/csmith-runtime-$compiler_label.stdout" \
    "$work_directory/tool-identities/csmith-runtime-$compiler_label.stderr" \
    "$runtime_probe_commands" \
    "$work_directory/tool-identities/csmith-runtime-$compiler_label.status" \
    "$compiler_path" -std=c11 -pedantic-errors -fsyntax-only \
    -I "$csmith_runtime" "$runtime_probe"
  status_is_zero \
    "$work_directory/tool-identities/csmith-runtime-$compiler_label.status" ||
    die "Csmith runtime is not valid strict C11 under $compiler_label"
done

csmith_version_output=$(LC_ALL=C "$csmith" --version) ||
  die "Csmith did not report its version"
if ! printf '%s\n' "$csmith_version_output" |
  grep -Fxq "csmith $csmith_version"; then
  die "Csmith executable is not the pinned version $csmith_version"
fi
{
  printf 'executable=%s\n' "$csmith"
  openssl dgst -sha256 "$csmith"
  printf '%s\n' '--version:' "$csmith_version_output"
} >"$work_directory/tool-identities/csmith.txt"
{
  printf 'executable=%s\n' "$csmith"
  find "$csmith_runtime" -type f -print | LC_ALL=C sort | while IFS= read -r header; do
    openssl dgst -sha256 "$header"
  done
} >"$work_directory/tool-identities/csmith-runtime.txt"
{
  printf 'executable=%s\n' "$ccc"
  printf 'native_driver=%s\n' "$CCC_CC"
  printf 'object_copier=%s\n' "$CCC_OBJCOPY"
  openssl dgst -sha256 "$ccc"
  git -C "$repository" rev-parse HEAD 2>/dev/null || true
  git -C "$repository" status --short 2>/dev/null || true
} >"$work_directory/tool-identities/ccc.txt"
cp -p "$ccc" "$work_directory/tool-identities/ccc-executable"
mkdir -p -- "$work_directory/tool-identities/ccc-resource-dir"
cp -R "$ccc_resource_dir/." \
  "$work_directory/tool-identities/ccc-resource-dir/"
find "$work_directory/tool-identities/ccc-resource-dir" -type f -print |
  LC_ALL=C sort | while IFS= read -r resource_file; do
    openssl dgst -sha256 "$resource_file"
  done >"$work_directory/tool-identities/ccc-resource-dir.txt"
git -C "$repository" diff --binary HEAD \
  >"$work_directory/tool-identities/ccc-source.patch" 2>/dev/null || true

reference_labels=()
reference_compilers=()
reference_options=()
for optimization in "${reference_optimizations[@]}"; do
  normalized=${optimization#-}
  normalized=$(printf '%s' "$normalized" | tr '[:upper:]' '[:lower:]')
  reference_labels+=("gcc-$normalized")
  reference_compilers+=("$reference_gcc")
  reference_options+=("$optimization")
done
for optimization in "${reference_optimizations[@]}"; do
  normalized=${optimization#-}
  normalized=$(printf '%s' "$normalized" | tr '[:upper:]' '[:lower:]')
  reference_labels+=("clang-$normalized")
  reference_compilers+=("$reference_clang")
  reference_options+=("$optimization")
done

generator_source=pinned-archive
generator_revision=$csmith_revision
if ((allow_unverified_csmith == 1)); then
  generator_source=developer-override
  generator_revision=unverified
fi
{
  printf 'target=x86_64-unknown-linux-gnu\n'
  printf 'language_mode=c11\n'
  printf 'csmith_version=%s\n' "$csmith_version"
  printf 'generator_revision=%s\n' "$generator_revision"
  printf 'profile_pinned_revision=%s\n' "$csmith_revision"
  printf 'requested_cases=%s\n' "$cases"
  printf 'start_seed=%s\n' "$start_seed"
  printf 'maximum_attempts=%s\n' "$max_attempts"
  printf 'last_possible_seed=%s\n' "$last_possible_seed"
  printf 'generator_source=%s\n' "$generator_source"
  printf 'ccc_native_driver=%s\n' "$CCC_CC"
  printf 'ccc_object_copier=%s\n' "$CCC_OBJCOPY"
  printf 'pinned_archive_sha256=%s\n' "$csmith_archive_sha256"
  printf 'pinned_archive_sha3_256=%s\n' "$csmith_archive_sha3_256"
  printf 'generator_timeout_seconds=%s\n' "$generator_timeout"
  printf 'compile_timeout_seconds=%s\n' "$compile_timeout"
  printf 'execution_timeout_seconds=%s\n' "$execution_timeout"
  printf 'generator_options='
  printf ' %q' "${generator_options[@]}"
  printf '\nreference_matrix='
  printf ' %s' "${reference_labels[@]}"
  printf '\n'
} >"$work_directory/run-config.txt"

printf 'seed\toutcome\tadmissible\tcompleted\tfailure\tdetail\tcase_directory\n' \
  >"$work_directory/summary.tsv"
attempted=0
admissible_total=0
completed_total=0
passed=0
failed=0
inadmissible_total=0
inconclusive_total=0
last_seed=

for ((index = 0; index < max_attempts; index++)); do
  ((completed_total >= cases)) && break
  seed=$((start_seed + index))
  case_directory=$(printf '%s/cases/case-%06d-seed-%s' \
    "$work_directory" "$((index + 1))" "$seed")
  mkdir -p -- "$case_directory"
  (
    set -euo pipefail
    run_case "$seed" "$case_directory"
  ) >"$case_directory/harness.log" 2>&1 &
  case_worker=$!
  if wait "$case_worker"; then
    case_status=0
  else
    case_status=$?
  fi
  if ((case_status != 0)) || ! valid_result "$case_directory/result.txt"; then
    write_result "$case_directory" harness-failure \
      "case worker exited with status $case_status without a valid result for seed $seed" \
      0 0 1 >>"$case_directory/harness.log" 2>&1
  fi

  outcome=$(sed -n 's/^outcome=//p' "$case_directory/result.txt")
  detail=$(sed -n 's/^detail=//p' "$case_directory/result.txt")
  admissible=$(sed -n 's/^admissible=//p' "$case_directory/result.txt")
  completed=$(sed -n 's/^completed=//p' "$case_directory/result.txt")
  failure=$(sed -n 's/^failure=//p' "$case_directory/result.txt")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$seed" "$outcome" "$admissible" "$completed" "$failure" \
    "$detail" "$case_directory" \
    >>"$work_directory/summary.tsv"
  printf 'seed %s: %s\n' "$seed" "$outcome"
  ((attempted += 1))
  admissible_total=$((admissible_total + admissible))
  completed_total=$((completed_total + completed))
  failed=$((failed + failure))
  last_seed=$seed
  if [[ "$outcome" == pass ]]; then
    ((passed += 1))
  elif [[ "$outcome" == inadmissible ]]; then
    ((inadmissible_total += 1))
  elif [[ "$outcome" == inconclusive-* ]]; then
    ((inconclusive_total += 1))
  fi
done

coverage_shortfall=0
if ((completed_total < cases)); then
  coverage_shortfall=1
  ((failed += 1))
fi
{
  printf 'attempted=%s\n' "$attempted"
  printf 'admissible=%s\n' "$admissible_total"
  printf 'completed=%s\n' "$completed_total"
  printf 'requested=%s\n' "$cases"
  printf 'passed=%s\n' "$passed"
  printf 'failures=%s\n' "$failed"
  printf 'inadmissible=%s\n' "$inadmissible_total"
  printf 'inconclusive=%s\n' "$inconclusive_total"
  printf 'coverage_shortfall=%s\n' "$coverage_shortfall"
  printf 'last_attempted_seed=%s\n' "$last_seed"
} >"$work_directory/run-summary.txt"

printf 'Csmith differential suite: %d/%d completed, %d passed, %d failures; %d attempted\n' \
  "$completed_total" "$cases" "$passed" "$failed" "$attempted"
if ((coverage_shortfall)); then
  printf 'Coverage shortfall: exhausted %d attempts before completing %d cases\n' \
    "$max_attempts" "$cases" >&2
fi
printf 'Summary: %s\n' "$work_directory/summary.tsv"
((failed == 0))
