#!/usr/bin/env bash
set -euo pipefail

if [[ ${CCC_REQUIRE_TESTFLOAT_ORACLE:-} != 1 ]]; then
    echo "CCC_REQUIRE_TESTFLOAT_ORACLE=1 is required; extF80 differential evidence may not be skipped" >&2
    exit 2
fi
if [[ $# -ne 0 ]]; then
    echo "usage: $0" >&2
    exit 2
fi
if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
    echo "the extF80 TestFloat oracle requires a native x86-64 Linux host" >&2
    exit 3
fi

root=$(cd "$(dirname "$0")/../.." && pwd)
fixtures=$root/tests/target-oracle
manifest=$fixtures/berkeley-testfloat.toml
subject_dir=$fixtures/testfloat-subject
ccc_bin=${CCC_BIN:-$root/target/debug/ccc}
artifact_dir=${CCC_TESTFLOAT_ARTIFACTS:-${TMPDIR:-/tmp}/ccc-testfloat-oracle}
softfloat_archive=${CCC_SOFTFLOAT_ARCHIVE:-}
testfloat_archive=${CCC_TESTFLOAT_ARCHIVE:-}
mkdir -p "$artifact_dir"
exec 3>&1 4>&2
exec > "$artifact_dir/run.log" 2>&1

work_dir=
finish() {
    local exit_status=$?
    trap - EXIT
    if [[ -n $work_dir && -d $work_dir ]]; then
        rm -rf "$work_dir"
    fi
    if (( exit_status != 0 )); then
        tail -n 240 "$artifact_dir/run.log" >&4
    fi
    exit "$exit_status"
}
trap finish EXIT

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "missing required tool: $1" >&2
        exit 3
    fi
}

require_file() {
    if [[ ! -f $1 ]]; then
        echo "missing required file: $1" >&2
        exit 3
    fi
}

manifest_value() {
    local key=$1
    awk -F ' = ' -v key="$key" '
        $1 == key {
            value = $2
            sub(/^"/, "", value)
            sub(/"$/, "", value)
            print value
            found = 1
            exit
        }
        END { if (!found) exit 1 }
    ' "$manifest"
}

sha3_256() {
    "$openssl_tool" dgst -sha3-256 "$1" | awk '{print $NF}'
}

verify_archive() {
    local label=$1 path=$2 bytes=$3 sha256=$4 sha3=$5
    local actual_bytes actual_sha256 actual_sha3

    require_file "$path"
    actual_bytes=$(wc -c < "$path" | tr -d ' ')
    actual_sha256=$(sha256sum "$path" | awk '{print $1}')
    actual_sha3=$(sha3_256 "$path")
    [[ $actual_bytes == "$bytes" ]] || {
        echo "$label archive byte length mismatch: expected $bytes, got $actual_bytes" >&2
        exit 4
    }
    [[ $actual_sha256 == "$sha256" ]] || {
        echo "$label archive SHA-256 mismatch" >&2
        exit 4
    }
    [[ $actual_sha3 == "$sha3" ]] || {
        echo "$label archive SHA3-256 mismatch" >&2
        exit 4
    }
}

for tool in awk cp gcc grep make mktemp nm openssl readelf rm sha256sum tail tee timeout tr uname unzip wc; do
    require_tool "$tool"
done
openssl_tool=$(command -v openssl)
require_file "$manifest"
require_file "$subject_dir/subjfloat.c"
require_file "$subject_dir/subjfloat_config.h"
require_file "$ccc_bin"
if [[ ! -x $ccc_bin ]]; then
    echo "CCC_BIN is not executable: $ccc_bin" >&2
    exit 3
fi
if [[ -z $softfloat_archive || -z $testfloat_archive ]]; then
    echo "CCC_SOFTFLOAT_ARCHIVE and CCC_TESTFLOAT_ARCHIVE must name the pinned archives" >&2
    exit 3
fi
[[ $(manifest_value role) == independent-test-harness-oracle ]]
[[ $(manifest_value production_linkage) == forbidden ]]
case $(gcc -dumpmachine) in
    x86_64*-linux-gnu*) ;;
    *)
        echo "GCC does not report an x86-64 Linux GNU target: $(gcc -dumpmachine)" >&2
        exit 3
        ;;
esac

verify_archive \
    SoftFloat \
    "$softfloat_archive" \
    "$(manifest_value softfloat_archive_bytes)" \
    "$(manifest_value softfloat_archive_sha256)" \
    "$(manifest_value softfloat_archive_sha3_256)"
verify_archive \
    TestFloat \
    "$testfloat_archive" \
    "$(manifest_value testfloat_archive_bytes)" \
    "$(manifest_value testfloat_archive_sha256)" \
    "$(manifest_value testfloat_archive_sha3_256)"

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-testfloat.XXXXXX")
unzip -q "$softfloat_archive" -d "$work_dir"
unzip -q "$testfloat_archive" -d "$work_dir"
softfloat_root=$work_dir/SoftFloat-3e
testfloat_root=$work_dir/TestFloat-3e
softfloat_build=$softfloat_root/build/Linux-x86_64-GCC
testfloat_build=$testfloat_root/build/Linux-x86_64-GCC
require_file "$softfloat_root/COPYING.txt"
require_file "$testfloat_root/COPYING.txt"

softfloat_license_sha=$(sha256sum "$softfloat_root/COPYING.txt" | awk '{print $1}')
testfloat_license_sha=$(sha256sum "$testfloat_root/COPYING.txt" | awk '{print $1}')
[[ $softfloat_license_sha == "$(manifest_value softfloat_license_sha256)" ]]
[[ $testfloat_license_sha == "$(manifest_value testfloat_license_sha256)" ]]
cp "$softfloat_root/COPYING.txt" "$artifact_dir/SoftFloat-COPYING.txt"
cp "$testfloat_root/COPYING.txt" "$artifact_dir/TestFloat-COPYING.txt"

{
    echo "role=$(manifest_value role)"
    echo "production_linkage=$(manifest_value production_linkage)"
    echo "softfloat_origin=$(manifest_value softfloat_origin)"
    echo "softfloat_archive_bytes=$(manifest_value softfloat_archive_bytes)"
    echo "softfloat_archive_sha256=$(manifest_value softfloat_archive_sha256)"
    echo "softfloat_archive_sha3_256=$(manifest_value softfloat_archive_sha3_256)"
    echo "softfloat_license_sha256=$softfloat_license_sha"
    echo "testfloat_origin=$(manifest_value testfloat_origin)"
    echo "testfloat_archive_bytes=$(manifest_value testfloat_archive_bytes)"
    echo "testfloat_archive_sha256=$(manifest_value testfloat_archive_sha256)"
    echo "testfloat_archive_sha3_256=$(manifest_value testfloat_archive_sha3_256)"
    echo "testfloat_license_sha256=$testfloat_license_sha"
    echo "generator_seed=$(manifest_value generator_seed)"
    echo "generator_level=$(manifest_value generator_level)"
    echo "rounding_precision=$(manifest_value rounding_precision)"
    echo "tininess_detection=$(manifest_value tininess_detection)"
    echo "subject_source_sha256=$(sha256sum "$subject_dir/subjfloat.c" | awk '{print $1}')"
    echo "subject_config_sha256=$(sha256sum "$subject_dir/subjfloat_config.h" | awk '{print $1}')"
} > "$artifact_dir/source-provenance.txt"

{
    uname -a
    "$ccc_bin" --version
    gcc --version
    gcc -dumpmachine
    make --version
    openssl version
    unzip -v
} > "$artifact_dir/tool-identities.txt" 2>&1

jobs=${CCC_TESTFLOAT_JOBS:-2}
make -j"$jobs" -C "$softfloat_build"
testfloat_opts="-DFLOAT64 -DEXTFLOAT80 -DLONG_DOUBLE_IS_EXTFLOAT80"
make -j"$jobs" -C "$testfloat_build" \
    "SUBJ_SOURCE_DIR=$subject_dir" \
    "TESTFLOAT_OPTS=$testfloat_opts" \
    testfloat.a subjfloat_functions.o testfloat.o

testfloat_bin=$testfloat_build/testfloat
subject_object=$testfloat_build/subjfloat.o
subject_function_table=$testfloat_build/subjfloat_functions.o
testfloat_main=$testfloat_build/testfloat.o
testfloat_library=$testfloat_build/testfloat.a
softfloat_library=$softfloat_build/softfloat.a
require_file "$subject_function_table"
require_file "$testfloat_main"
require_file "$testfloat_library"
require_file "$softfloat_library"

operations=(
    ui64_to_extF80
    i64_to_extF80
    f32_to_extF80
    f64_to_extF80
    extF80_to_ui64_rx_minMag
    extF80_to_i64_rx_minMag
    extF80_to_f32
    extF80_to_f64
    extF80_add
    extF80_sub
    extF80_mul
    extF80_div
    extF80_eq
    extF80_le
    extF80_lt
)
expected_cases=(
    756 756 600 768 912 912 3648 3648
    185856 185856 185856 185856 46464 46464 46464
)
expected_groups=(
    1 1 1 1 1 1 4 4 4 4 4 4 1 1 1
)
optimizations=(-O0 -O2)
seed=$(manifest_value generator_seed)
expected_per_optimization=$(manifest_value expected_cases_per_optimization)
expected_total=$(manifest_value expected_cases_total)
observed_total=0

for optimization in "${optimizations[@]}"; do
    suffix=${optimization#-}
    compile_log=$artifact_dir/subject-$suffix.compile.log
    "$ccc_bin" \
        --target=x86_64-unknown-linux-gnu \
        -march=x86-64 \
        -mcpu=generic \
        -mabi=lp64 \
        -std=gnu11 \
        "$optimization" \
        -I "$testfloat_build" \
        -I "$testfloat_root/source" \
        -I "$softfloat_root/source/include" \
        -c "$subject_dir/subjfloat.c" \
        -o "$subject_object" \
        > "$compile_log" 2>&1

    readelf -h "$subject_object" > "$artifact_dir/subject-$suffix.elf-header.txt"
    grep -Eq 'Class: +ELF64' "$artifact_dir/subject-$suffix.elf-header.txt"
    grep -Eq 'Machine: +Advanced Micro Devices X86-64' "$artifact_dir/subject-$suffix.elf-header.txt"
    nm -g --defined-only "$subject_object" > "$artifact_dir/subject-$suffix.symbols.txt"
    nm -u "$subject_object" > "$artifact_dir/subject-$suffix.undefined-symbols.txt"
    if grep -Eiq 'softfloat' "$artifact_dir/subject-$suffix.undefined-symbols.txt"; then
        echo "CCC's subject object unexpectedly depends on SoftFloat" >&2
        exit 5
    fi
    for symbol in \
        subj_ui64_to_extF80M subj_i64_to_extF80M \
        subj_f32_to_extF80M subj_f64_to_extF80M \
        subj_extF80M_to_ui64_rx_minMag subj_extF80M_to_i64_rx_minMag \
        subj_extF80M_to_f32 subj_extF80M_to_f64 \
        subj_extF80M_add subj_extF80M_sub subj_extF80M_mul subj_extF80M_div \
        subj_extF80M_eq subj_extF80M_le subj_extF80M_lt; do
        grep -Eq " [Tt] $symbol$" "$artifact_dir/subject-$suffix.symbols.txt"
    done

    subject_sha=$(sha256sum "$subject_object" | awk '{print $1}')
    gcc -o "$testfloat_bin" \
        "$subject_object" \
        "$subject_function_table" \
        "$testfloat_main" \
        "$testfloat_library" \
        "$softfloat_library" \
        -lm \
        > "$artifact_dir/link-$suffix.log" 2>&1
    linked_subject_sha=$(sha256sum "$subject_object" | awk '{print $1}')
    [[ $subject_sha == "$linked_subject_sha" ]] || {
        echo "the TestFloat link changed CCC's subject object" >&2
        exit 5
    }
    cp "$testfloat_bin" "$artifact_dir/testfloat-$suffix"

    "$testfloat_bin" -list > "$artifact_dir/operations-$suffix.txt"
    operation_count=$(wc -l < "$artifact_dir/operations-$suffix.txt" | tr -d ' ')
    [[ $operation_count == "${#operations[@]}" ]] || {
        echo "TestFloat exposed $operation_count subject operations; expected ${#operations[@]}" >&2
        exit 5
    }
    for operation in "${operations[@]}"; do
        grep -Fxq "$operation" "$artifact_dir/operations-$suffix.txt"
    done

    observed_optimization=0
    for index in "${!operations[@]}"; do
        operation=${operations[$index]}
        operation_log=$artifact_dir/$suffix-$operation.log
        if ! timeout 120 "$testfloat_bin" \
                -level 1 \
                -seed "$seed" \
                -precision80 \
                -tininessafter \
                -errors 8 \
                -errorstop \
                "$operation" \
                2>&1 | tr '\r' '\n' > "$operation_log"; then
            echo "TestFloat execution failed for $optimization $operation" >&2
            tail -n 80 "$operation_log" >&2
            exit 6
        fi

        if grep -Fq 'Errors found' "$operation_log"; then
            echo "TestFloat reported a discrepancy for $optimization $operation" >&2
            tail -n 80 "$operation_log" >&2
            exit 6
        fi
        read -r cases groups < <(
            awk '/tests total/ { cases += $1; groups += 1 }
                 END { print cases + 0, groups + 0 }' "$operation_log"
        )
        [[ $cases == "${expected_cases[$index]}" ]] || {
            echo "$optimization $operation executed $cases cases; expected ${expected_cases[$index]}" >&2
            exit 6
        }
        [[ $groups == "${expected_groups[$index]}" ]] || {
            echo "$optimization $operation executed $groups groups; expected ${expected_groups[$index]}" >&2
            exit 6
        }
        if (( groups == 4 )); then
            for rounding_mode in near_even minMag min max; do
                grep -Fq "rounding $rounding_mode." "$operation_log" || {
                    echo "$optimization $operation omitted rounding mode $rounding_mode" >&2
                    exit 6
                }
            done
        fi
        no_error_groups=$(grep -c 'no errors found' "$operation_log" || true)
        [[ $no_error_groups == "$groups" ]] || {
            echo "$optimization $operation did not report a clean result for every group" >&2
            exit 6
        }
        observed_optimization=$((observed_optimization + cases))
        printf 'PASS %s %s (%d cases in %d group(s))\n' \
            "$optimization" "$operation" "$cases" "$groups"
    done
    [[ $observed_optimization == "$expected_per_optimization" ]] || {
        echo "$optimization executed $observed_optimization total cases; expected $expected_per_optimization" >&2
        exit 6
    }
    observed_total=$((observed_total + observed_optimization))
done

[[ $observed_total == "$expected_total" ]] || {
    echo "executed $observed_total total cases; expected $expected_total" >&2
    exit 6
}
printf 'PASS Berkeley TestFloat checked %d CCC extF80 cases\n' "$observed_total" | tee "$artifact_dir/summary.txt" >&3
