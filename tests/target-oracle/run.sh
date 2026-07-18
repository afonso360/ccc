#!/usr/bin/env bash
set -euo pipefail

if [[ ${CCC_REQUIRE_TARGET_ORACLE:-} != 1 ]]; then
    echo "CCC_REQUIRE_TARGET_ORACLE=1 is required; target evidence may not be skipped" >&2
    exit 2
fi
if [[ $# -ne 1 ]]; then
    echo "usage: $0 aarch64-linux|riscv64-linux|darwin-arm64" >&2
    exit 2
fi

target_name=$1
root=$(cd "$(dirname "$0")/../.." && pwd)
fixtures=$root/tests/target-oracle
ccc_bin=${CCC_BIN:-$root/target/debug/ccc}
artifact_dir=${CCC_TARGET_ORACLE_ARTIFACTS:-${TMPDIR:-/tmp}/ccc-target-oracle-$target_name}
mkdir -p "$artifact_dir"
exec 3>&1 4>&2
exec > "$artifact_dir/run.log" 2>&1
show_log_on_failure() {
    local status=$1
    if (( status != 0 )); then
        tail -n 240 "$artifact_dir/run.log" >&4
    fi
    trap - EXIT
    exit "$status"
}
trap 'show_log_on_failure "$?"' EXIT

case_count=0
pass() {
    case_count=$((case_count + 1))
    printf 'PASS [%02d] %s\n' "$case_count" "$1"
}

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

require_tool tee
require_tool cmp
require_file "$ccc_bin"
if [[ ! -x $ccc_bin ]]; then
    echo "CCC_BIN is not executable: $ccc_bin" >&2
    exit 3
fi

platform=
triple=
object_copier=
macho_localizer=
qemu_root=
declare -a ccc_min ccc_full reference_cc runner

case $target_name in
    aarch64-linux)
        platform=elf
        triple=aarch64-unknown-linux-gnu
        reference_driver=aarch64-linux-gnu-gcc
        object_copier=aarch64-linux-gnu-objcopy
        readelf_tool=aarch64-linux-gnu-readelf
        nm_tool=aarch64-linux-gnu-nm
        objdump_tool=aarch64-linux-gnu-objdump
        qemu_tool=qemu-aarch64
        qemu_root=${CCC_QEMU_ROOT:-/usr/aarch64-linux-gnu}
        ccc_min=("$ccc_bin" "--target=$triple")
        ccc_full=("$ccc_bin" "--target=$triple" -march=armv8-a -mcpu=generic -mabi=lp64)
        reference_cc=("$reference_driver" -march=armv8-a -mabi=lp64)
        ;;
    riscv64-linux)
        platform=elf
        triple=riscv64-unknown-linux-gnu
        reference_driver=riscv64-linux-gnu-gcc
        object_copier=riscv64-linux-gnu-objcopy
        readelf_tool=riscv64-linux-gnu-readelf
        nm_tool=riscv64-linux-gnu-nm
        objdump_tool=riscv64-linux-gnu-objdump
        qemu_tool=qemu-riscv64
        qemu_root=${CCC_QEMU_ROOT:-/usr/riscv64-linux-gnu}
        ccc_min=("$ccc_bin" "--target=$triple")
        ccc_full=("$ccc_bin" "--target=$triple" -march=rv64gc -mcpu=generic -mabi=lp64d)
        reference_cc=("$reference_driver" -march=rv64gc -mabi=lp64d)
        ;;
    darwin-arm64)
        platform=macho
        triple=aarch64-apple-darwin
        if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
            echo "darwin-arm64 evidence requires a native arm64 Darwin host" >&2
            exit 3
        fi
        for tool in xcrun otool nm dwarfdump lldb file shasum; do require_tool "$tool"; done
        sdk_root=${CCC_DARWIN_SDK_ROOT:-$(xcrun --sdk macosx --show-sdk-path)}
        require_file "$sdk_root/usr/include/stdio.h"
        reference_driver=${CCC_DARWIN_CC:-/usr/bin/cc}
        require_file "$reference_driver"
        macho_localizer=${CCC_NMEDIT:-$(xcrun --find nmedit)}
        if [[ -z $macho_localizer || ! -x $macho_localizer ]]; then
            echo "CCC_NMEDIT must name Apple's nmedit for exact Mach-O symbol localization" >&2
            exit 3
        fi
        if [[ $(basename "$macho_localizer") != nmedit ]]; then
            echo "CCC_NMEDIT does not resolve to nmedit: $macho_localizer" >&2
            exit 3
        fi
        ccc_min=("$ccc_bin" "--target=$triple" "--sdk-root=$sdk_root" -mmacosx-version-min=11.0)
        ccc_full=("$ccc_bin" "--target=$triple" -march=armv8-a -mcpu=generic -mabi=darwin "--sdk-root=$sdk_root" -mmacosx-version-min=11.0)
        reference_cc=("$reference_driver" -target arm64-apple-macos11 -isysroot "$sdk_root" -mmacosx-version-min=11.0)
        ;;
    *)
        echo "unknown target oracle: $target_name" >&2
        exit 2
        ;;
esac

require_tool "$reference_driver"

if [[ $platform == elf ]]; then
    require_tool "$object_copier"
    for tool in "$readelf_tool" "$nm_tool" "$objdump_tool" "$qemu_tool" gdb-multiarch timeout; do
        require_tool "$tool"
    done
    if [[ ! -d $qemu_root ]]; then
        echo "QEMU root does not exist: $qemu_root" >&2
        exit 3
    fi
    runner=("$qemu_tool" -L "$qemu_root")
else
    runner=()
fi

{
    echo "target=$target_name"
    echo "triple=$triple"
    "$ccc_bin" --help | head -n 1
    "${reference_cc[@]}" --version
    if [[ $platform == elf ]]; then
        "$object_copier" --version
        "$reference_driver" -dumpmachine
        "$reference_driver" -print-sysroot
        "$readelf_tool" --version
        "$qemu_tool" --version
        gdb-multiarch --version
        echo "qemu_root=$qemu_root"
    else
        echo "nmedit=$macho_localizer"
        shasum -a 256 "$macho_localizer"
        xcrun --show-sdk-version
        xcrun xcodebuild -version 2>/dev/null || true
        xcrun ld -version_details 2>/dev/null || xcrun ld -v
        lldb --version
        echo "sdk_root=$sdk_root"
        echo "deployment_target=11.0"
    fi
} > "$artifact_dir/tool-identities.txt" 2>&1

run_ccc() {
    if [[ $platform == macho ]]; then
        CCC_NMEDIT="$macho_localizer" "$@"
    else
        CCC_OBJCOPY="$object_copier" "$@"
    fi
}

compile_ccc() {
    local source=$1 output=$2 optimization=$3
    shift 3
    run_ccc "${ccc_full[@]}" "$optimization" -I "$fixtures" "$@" -c "$fixtures/$source" -o "$output"
}

compile_ccc_min() {
    local source=$1 output=$2
    run_ccc "${ccc_min[@]}" -O0 -nostdinc -I "$fixtures" -c "$fixtures/$source" -o "$output"
}

compile_ref() {
    local source=$1 output=$2 optimization=$3
    "${reference_cc[@]}" -std=gnu11 -Wall -Wextra -Werror "$optimization" -I "$fixtures" -c "$fixtures/$source" -o "$output"
}

link_ref() {
    local output=$1
    shift
    "${reference_cc[@]}" "$@" -o "$output"
}

run_executable() {
    local executable=$1
    if [[ $platform == elf ]]; then
        local interpreter
        interpreter=$($readelf_tool -l "$executable" | awk '/Requesting program interpreter/ { value=$NF; gsub(/\]/, "", value); print value; exit }')
        if [[ -z $interpreter || ! -e $qemu_root$interpreter ]]; then
            echo "target interpreter is absent from QEMU root: ${interpreter:-<none>}" >&2
            return 1
        fi
        "${runner[@]}" "$executable"
    else
        "$executable"
    fi
}

expect_ccc_failure() {
    local source=$1 code=$2
    local stem=${source%.c}
    local log=$artifact_dir/$stem.failure.txt
    if run_ccc "${ccc_full[@]}" -nostdinc -c "$fixtures/$source" -o "$artifact_dir/$stem.o" >"$log" 2>&1; then
        echo "$source unexpectedly compiled" >&2
        return 1
    fi
    grep -q "error\[$code\]" "$log"
    pass "$source rejects with $code"
}

for optimization in -O0 -O2; do
    suffix=${optimization#-}

    ccc_fixed=$artifact_dir/ccc-fixed-$suffix.o
    ref_caller=$artifact_dir/ref-calls-ccc-fixed-$suffix.o
    fixed_forward=$artifact_dir/fixed-ref-to-ccc-$suffix
    compile_ccc ccc_fixed_definitions.c "$ccc_fixed" "$optimization"
    compile_ref reference_calls_ccc.c "$ref_caller" "$optimization"
    link_ref "$fixed_forward" "$ref_caller" "$ccc_fixed"
    run_executable "$fixed_forward"
    pass "$optimization reference caller to CCC fixed ABI"

    ref_fixed=$artifact_dir/ref-fixed-$suffix.o
    ccc_caller=$artifact_dir/ccc-calls-ref-fixed-$suffix.o
    fixed_reverse=$artifact_dir/fixed-ccc-to-ref-$suffix
    compile_ref reference_fixed_definitions.c "$ref_fixed" "$optimization"
    compile_ccc ccc_calls_reference.c "$ccc_caller" "$optimization"
    link_ref "$fixed_reverse" "$ccc_caller" "$ref_fixed"
    run_executable "$fixed_reverse"
    pass "$optimization CCC caller to reference fixed ABI"

    ccc_variadic=$artifact_dir/ccc-variadic-$suffix.o
    ref_variadic_caller=$artifact_dir/ref-calls-ccc-variadic-$suffix.o
    variadic_forward=$artifact_dir/variadic-ref-to-ccc-$suffix
    compile_ccc ccc_variadic_definitions.c "$ccc_variadic" "$optimization"
    compile_ref reference_calls_ccc_variadic.c "$ref_variadic_caller" "$optimization"
    link_ref "$variadic_forward" "$ref_variadic_caller" "$ccc_variadic"
    run_executable "$variadic_forward"
    pass "$optimization reference caller to CCC variadic ABI and va_copy"

    ref_variadic=$artifact_dir/ref-variadic-$suffix.o
    ccc_variadic_caller=$artifact_dir/ccc-calls-ref-variadic-$suffix.o
    variadic_reverse=$artifact_dir/variadic-ccc-to-ref-$suffix
    compile_ref reference_variadic_definitions.c "$ref_variadic" "$optimization"
    compile_ccc ccc_calls_reference_variadic.c "$ccc_variadic_caller" "$optimization"
    link_ref "$variadic_reverse" "$ccc_variadic_caller" "$ref_variadic"
    run_executable "$variadic_reverse"
    pass "$optimization CCC caller to reference variadic ABI"

    ccc_libc=$artifact_dir/ccc-libc-variadic-$suffix.o
    ref_libc_caller=$artifact_dir/ref-calls-ccc-libc-variadic-$suffix.o
    libc_variadic=$artifact_dir/libc-variadic-$suffix
    compile_ccc ccc_libc_variadic.c "$ccc_libc" "$optimization"
    compile_ref reference_calls_ccc_libc_variadic.c "$ref_libc_caller" "$optimization"
    link_ref "$libc_variadic" "$ref_libc_caller" "$ccc_libc"
    run_executable "$libc_variadic"
    pass "$optimization CCC-created va_list consumed by libc vsnprintf"

    header_executable=$artifact_dir/header-sentinel-$suffix
    run_ccc "${ccc_full[@]}" "$optimization" -I "$fixtures" "$fixtures/header_sentinel.c" -o "$header_executable"
    run_executable "$header_executable"
    pass "$optimization CCC installed hosted headers compile, link, and execute"
    reference_header_executable=$artifact_dir/reference-header-sentinel-$suffix
    "${reference_cc[@]}" -std=gnu11 -Wall -Wextra -Werror "$optimization" -I "$fixtures" "$fixtures/header_sentinel.c" -o "$reference_header_executable"
    run_executable "$reference_header_executable"
    pass "$optimization reference installed hosted headers compile, link, and execute"

    ccc_unwind=$artifact_dir/ccc-unwind-$suffix.o
    ref_unwind=$artifact_dir/ref-unwind-$suffix.o
    ref_unwind_main=$artifact_dir/ref-unwind-main-$suffix.o
    ccc_unwind_main=$artifact_dir/ccc-unwind-main-$suffix.o
    compile_ccc ccc_unwind_entry.c "$ccc_unwind" "$optimization"
    compile_ref reference_unwind_probe.c "$ref_unwind" "$optimization"
    compile_ref reference_calls_ccc_unwind.c "$ref_unwind_main" "$optimization"
    compile_ccc ccc_calls_reference_unwind.c "$ccc_unwind_main" "$optimization"
    unwind_entry=$artifact_dir/unwind-variadic-entry-$suffix
    unwind_helper=$artifact_dir/unwind-call-helper-$suffix
    link_ref "$unwind_entry" "$ref_unwind_main" "$ccc_unwind" "$ref_unwind"
    link_ref "$unwind_helper" "$ccc_unwind_main" "$ccc_unwind" "$ref_unwind"
    run_executable "$unwind_entry"
    run_executable "$unwind_helper"
    pass "$optimization unwind crosses fixed frames and both variadic bridges"
done

macro_explicit=$artifact_dir/macro-explicit.o
macro_normalized=$artifact_dir/macro-normalized.o
compile_ccc macro_sentinel.c "$macro_explicit" -O0 -nostdinc
compile_ccc_min macro_sentinel.c "$macro_normalized"
cmp "$macro_explicit" "$macro_normalized"
pass "implicit and explicit normalized target options emit identical objects"

declare -a macro_names
case $target_name in
    aarch64-linux)
        macro_names=(__SIZEOF_POINTER__ __SIZEOF_LONG__ __aarch64__ __ARM_ARCH __ARM_PCS_AAPCS64 __CHAR_UNSIGNED__ __LDBL_MANT_DIG__)
        ;;
    riscv64-linux)
        macro_names=(__SIZEOF_POINTER__ __SIZEOF_LONG__ __riscv __riscv_xlen __riscv_float_abi_double __riscv_cmodel_medany __riscv_cmodel_pic __riscv_zicsr __riscv_zifencei __LDBL_MANT_DIG__)
        ;;
    darwin-arm64)
        macro_names=(__SIZEOF_POINTER__ __SIZEOF_LONG__ __arm64__ __APPLE__ __APPLE_CC__ __LDBL_MANT_DIG__ __ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__)
        ;;
esac
run_reference_macro() {
    if [[ $target_name == riscv64-linux ]]; then
        "${reference_cc[@]}" -mcmodel=medany "$@"
    else
        "${reference_cc[@]}" "$@"
    fi
}
reference_macro_object=$artifact_dir/reference-macro-sentinel.o
run_reference_macro -std=gnu11 -Wall -Wextra -Werror -nostdinc -c "$fixtures/macro_sentinel.c" -o "$reference_macro_object"
ccc_macros=$artifact_dir/ccc-predefined-macros.txt
reference_macros=$artifact_dir/reference-predefined-macros.txt
run_ccc "${ccc_full[@]}" -nostdinc -dM -E "$fixtures/macro_sentinel.c" > "$ccc_macros"
run_reference_macro -nostdinc -dM -E "$fixtures/macro_sentinel.c" > "$reference_macros"
: > "$artifact_dir/selected-macro-comparison.txt"
for name in "${macro_names[@]}"; do
    ccc_value=$(awk -v name="$name" '$1 == "#define" && $2 == name { $1 = ""; $2 = ""; sub(/^[[:space:]]+/, ""); print; found = 1; exit } END { if (!found) exit 1 }' "$ccc_macros")
    reference_value=$(awk -v name="$name" '$1 == "#define" && $2 == name { $1 = ""; $2 = ""; sub(/^[[:space:]]+/, ""); print; found = 1; exit } END { if (!found) exit 1 }' "$reference_macros")
    if [[ $ccc_value != "$reference_value" ]]; then
        echo "predefined macro mismatch for $name: CCC=$ccc_value reference=$reference_value" >&2
        exit 1
    fi
    printf '%s=%s\n' "$name" "$ccc_value" >> "$artifact_dir/selected-macro-comparison.txt"
done
pass "predefined target identity matches the reference compiler profile"

symbol_object=$artifact_dir/symbol-contract.o
compile_ccc darwin_symbol_contract.c "$symbol_object" -O0 -nostdinc

if [[ $platform == elf ]]; then
    long_double_storage=$artifact_dir/long-double-storage.o
    compile_ccc long_double_storage.c "$long_double_storage" -O0 -nostdinc
    $readelf_tool -sW "$long_double_storage" | grep -q 'binary128_storage'
    pass "binary128 layout and uninitialized object storage remain available"
    expect_ccc_failure long_double_initialization.c CCC2343
    expect_ccc_failure long_double_conversion.c CCC2343
    expect_ccc_failure long_double_arithmetic.c CCC2343
    expect_ccc_failure long_double_boundary.c CCC2343
    expect_ccc_failure long_double_aggregate_boundary.c CCC3509
    expect_ccc_failure long_double_va_arg.c CCC2404
    expect_ccc_failure long_double_tls.c CCC2441

    $readelf_tool -h "$ccc_fixed" > "$artifact_dir/fixed.elf-header.txt"
    $readelf_tool -SW "$ccc_variadic" > "$artifact_dir/variadic.elf-sections.txt"
    $readelf_tool -rW "$ccc_variadic" > "$artifact_dir/variadic.elf-relocations.txt"
    $readelf_tool -sW "$ccc_variadic" > "$artifact_dir/variadic.elf-symbols.txt"
    grep -q 'ELF64' "$artifact_dir/fixed.elf-header.txt"
    if [[ $target_name == aarch64-linux ]]; then
        grep -q 'AArch64' "$artifact_dir/fixed.elf-header.txt"
    else
        grep -q 'RISC-V' "$artifact_dir/fixed.elf-header.txt"
        grep -Eq 'Flags:.*0x5([^0-9a-f]|$)' "$artifact_dir/fixed.elf-header.txt"
    fi
    grep -q '\.eh_frame' "$artifact_dir/variadic.elf-sections.txt"
    grep -Eq '\.rela?\.eh_frame' "$artifact_dir/variadic.elf-relocations.txt"
    grep -q 'ccc_collect' "$artifact_dir/variadic.elf-symbols.txt"
    grep -q '__ccc_' "$artifact_dir/variadic.elf-symbols.txt"
    grep -q 'collect_copy' "$artifact_dir/variadic.elf-relocations.txt"
    grep -q '__ccc_variadic_body_' "$artifact_dir/variadic.elf-relocations.txt"
    if [[ $target_name == aarch64-linux ]]; then
        grep -q 'R_AARCH64_CALL26' "$artifact_dir/variadic.elf-relocations.txt"
    else
        grep -q 'R_RISCV_CALL_PLT' "$artifact_dir/variadic.elf-relocations.txt"
    fi
    grep -q '\.note.GNU-stack' "$artifact_dir/variadic.elf-sections.txt"
    if grep '\.note.GNU-stack' "$artifact_dir/variadic.elf-sections.txt" | grep -q ' X '; then
        echo "packaged object requests an executable stack" >&2
        exit 1
    fi
    $readelf_tool -sW "$symbol_object" > "$artifact_dir/symbol-contract.elf-symbols.txt"
    grep -Eq 'GLOBAL +HIDDEN .*hidden_global' "$artifact_dir/symbol-contract.elf-symbols.txt"
    grep -Eq 'LOCAL +DEFAULT .*internal_variadic' "$artifact_dir/symbol-contract.elf-symbols.txt"
    grep -Eq 'COM +.*tentative_global|COMMON +.*tentative_global' "$artifact_dir/symbol-contract.elf-symbols.txt"
    $readelf_tool -h "$fixed_forward" > "$artifact_dir/final.elf-header.txt"
    $readelf_tool -dW "$fixed_forward" > "$artifact_dir/final.elf-dynamic.txt"
    grep -Eq 'Type:.*DYN' "$artifact_dir/final.elf-header.txt"
    grep -Eq 'FLAGS_1.*PIE' "$artifact_dir/final.elf-dynamic.txt"
    if grep -q TEXTREL "$artifact_dir/final.elf-dynamic.txt"; then
        echo "final PIE contains text relocations" >&2
        exit 1
    fi
    pass "ELF machine, flags, CFI, stack, symbols, relocations, and PIE contract"
else
    ccc_long_double=$artifact_dir/ccc-darwin-long-double.o
    ref_long_double=$artifact_dir/ref-calls-ccc-darwin-long-double.o
    long_double_executable=$artifact_dir/darwin-long-double
    compile_ccc darwin_long_double.c "$ccc_long_double" -O0 -nostdinc
    compile_ref reference_calls_ccc_darwin_long_double.c "$ref_long_double" -O0
    link_ref "$long_double_executable" "$ref_long_double" "$ccc_long_double"
    run_executable "$long_double_executable"
    pass "Darwin binary64 long double crosses the Apple ABI"

    file "$ccc_variadic" > "$artifact_dir/variadic.macho-file.txt"
    otool -hv "$ccc_variadic" > "$artifact_dir/variadic.macho-header.txt"
    otool -l "$ccc_variadic" > "$artifact_dir/variadic.macho-load-commands.txt"
    otool -rv "$ccc_variadic" > "$artifact_dir/variadic.macho-relocations.txt"
    nm -m "$ccc_variadic" > "$artifact_dir/variadic.macho-symbols.txt"
    dwarfdump --eh-frame "$ccc_fixed" > "$artifact_dir/fixed.macho-eh-frame.txt"
    dwarfdump --eh-frame "$variadic_forward" > "$artifact_dir/variadic-final.macho-eh-frame.txt"
    grep -qi 'Mach-O 64-bit.*arm64' "$artifact_dir/variadic.macho-file.txt"
    grep -Eq 'ARM64.*OBJECT' "$artifact_dir/variadic.macho-header.txt"
    grep -A8 'LC_BUILD_VERSION' "$artifact_dir/variadic.macho-load-commands.txt" | grep -q 'minos 11.0'
    grep -q 'FDE cie=' "$artifact_dir/fixed.macho-eh-frame.txt"
    grep -q 'FDE cie=' "$artifact_dir/variadic-final.macho-eh-frame.txt"
    grep -q '_ccc_collect' "$artifact_dir/variadic.macho-symbols.txt"
    grep -q '___ccc_' "$artifact_dir/variadic.macho-symbols.txt"
    grep -Eq '\) non-external .*___ccc_' "$artifact_dir/variadic.macho-symbols.txt"
    grep -Eq 'BR26 .*___ccc_variadic_body_' "$artifact_dir/variadic.macho-relocations.txt"
    grep -Eq 'SUB .*func\.eh' "$artifact_dir/variadic.macho-relocations.txt"
    grep -Eq 'UNSIGND .*_ccc_collect' "$artifact_dir/variadic.macho-relocations.txt"
    if awk '/___ccc_/ && /\) external / { found = 1 } END { exit found ? 0 : 1 }' "$artifact_dir/variadic.macho-symbols.txt"; then
        echo "generated Mach-O bridge symbol remained external" >&2
        exit 1
    fi
    nm -m "$symbol_object" > "$artifact_dir/symbol-contract.macho-symbols.txt"
    otool -l "$symbol_object" > "$artifact_dir/symbol-contract.macho-load-commands.txt"
    grep -Eq '\) external _tentative_global$' "$artifact_dir/symbol-contract.macho-symbols.txt"
    grep -Eq '\) private external _hidden_global$' "$artifact_dir/symbol-contract.macho-symbols.txt"
    grep -Eq '\) non-external .*_internal_variadic$' "$artifact_dir/symbol-contract.macho-symbols.txt"
    grep -q '__DATA,__bss' "$artifact_dir/symbol-contract.macho-symbols.txt"
    otool -hv "$fixed_forward" > "$artifact_dir/final.macho-header.txt"
    grep -q 'PIE' "$artifact_dir/final.macho-header.txt"
    pass "Mach-O build version, CFI, symbols, visibility, relocation, and PIE contract"
fi

debugger_entry=$artifact_dir/unwind-variadic-entry-O0
debugger_helper=$artifact_dir/unwind-call-helper-O0
if [[ $platform == elf ]]; then
    helper_symbol=$($nm_tool "$debugger_helper" | awk '/__ccc_call_helper/ { print $3; exit }')
    if [[ -z $helper_symbol ]]; then
        echo "no generated call-helper symbol in debugger fixture" >&2
        exit 1
    fi
    port_base=${CCC_GDB_PORT_BASE:-$((30000 + ($$ % 1000) * 2))}
    run_gdb_probe() (
        local executable=$1 symbol=$2 port=$3 stem=$4
        "$qemu_tool" -L "$qemu_root" -g "$port" "$executable" > "$artifact_dir/qemu-$stem.txt" 2>&1 &
        local qemu_pid=$!
        trap 'kill "$qemu_pid" 2>/dev/null || true' EXIT
        timeout 30 gdb-multiarch -q -batch \
            -ex "file $executable" -ex "target remote :$port" \
            -ex "break $symbol" -ex continue -ex bt -ex detach \
            > "$artifact_dir/gdb-$stem.txt" 2>&1
        wait "$qemu_pid"
    )
    run_gdb_probe "$debugger_entry" ccc_unwind_variadic "$port_base" variadic-entry
    run_gdb_probe "$debugger_helper" "$helper_symbol" "$((port_base + 1))" call-helper
    grep -q 'ccc_unwind_variadic' "$artifact_dir/gdb-variadic-entry.txt"
    grep -q 'main' "$artifact_dir/gdb-variadic-entry.txt"
    grep -q "$helper_symbol" "$artifact_dir/gdb-call-helper.txt"
    grep -q 'main' "$artifact_dir/gdb-call-helper.txt"
else
    helper_symbol=$(nm "$debugger_helper" | awk '/___ccc_call_helper/ { print $3; exit }')
    if [[ -z $helper_symbol ]]; then
        echo "no generated call-helper symbol in debugger fixture" >&2
        exit 1
    fi
    run_lldb_probe() {
        local executable=$1 symbol=$2 stem=$3
        local log=$artifact_dir/lldb-$stem.txt
        local timed_out=$artifact_dir/lldb-$stem.timeout
        local lldb_pid watchdog_pid status
        rm -f "$timed_out"
        lldb --batch -o "target create $executable" \
            -o "breakpoint set --name $symbol" -o run -o bt > "$log" 2>&1 &
        lldb_pid=$!
        (
            sleep 30
            if kill -0 "$lldb_pid" 2>/dev/null; then
                : > "$timed_out"
                kill "$lldb_pid" 2>/dev/null || true
                sleep 2
                kill -9 "$lldb_pid" 2>/dev/null || true
            fi
        ) &
        watchdog_pid=$!
        if wait "$lldb_pid"; then status=0; else status=$?; fi
        kill "$watchdog_pid" 2>/dev/null || true
        wait "$watchdog_pid" 2>/dev/null || true
        if [[ -f $timed_out ]]; then
            echo "LLDB timed out after 30 seconds while probing $symbol" >> "$log"
            return 124
        fi
        return "$status"
    }
    run_lldb_probe "$debugger_entry" ccc_unwind_variadic variadic-entry
    run_lldb_probe "$debugger_helper" "$helper_symbol" call-helper
    grep -q 'ccc_unwind_variadic' "$artifact_dir/lldb-variadic-entry.txt"
    grep -q 'main' "$artifact_dir/lldb-variadic-entry.txt"
    grep -q "$helper_symbol" "$artifact_dir/lldb-call-helper.txt"
    grep -q 'main' "$artifact_dir/lldb-call-helper.txt"
fi
pass "debugger stops in the variadic entry and generated call helper with caller frames"

if [[ $platform == elf ]]; then
    expected_cases=28
else
    expected_cases=21
fi
if (( case_count != expected_cases )); then
    echo "target oracle ran $case_count cases; expected exactly $expected_cases" >&2
    exit 1
fi
printf 'Target oracle complete: %d checks for %s\n' "$case_count" "$target_name"
cat "$artifact_dir/run.log" >&3
trap - EXIT
