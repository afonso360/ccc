#!/usr/bin/env bash

# This file is sourced by run.sh and test-case-plan.sh. Keep the case names
# independent from the executable oracle sequence: run.sh must report each
# completed case in this exact order.

declare -a TARGET_ORACLE_CASE_PLAN

target_oracle_build_case_plan() {
    local target_name=$1
    local optimization

    case $target_name in
        x86_64-linux|aarch64-linux|riscv64-linux|darwin-arm64)
            ;;
        *)
            echo "unknown target oracle case plan: $target_name" >&2
            return 2
            ;;
    esac

    TARGET_ORACLE_CASE_PLAN=()
    for optimization in -O0 -O2; do
        TARGET_ORACLE_CASE_PLAN+=(
            "$optimization reference caller to CCC fixed ABI"
            "$optimization CCC caller to reference fixed ABI"
            "$optimization reference caller to CCC variadic ABI and va_copy"
            "$optimization CCC caller to reference variadic ABI"
            "$optimization CCC-created va_list consumed by libc vsnprintf"
            "$optimization CCC installed hosted headers compile, link, and execute"
            "$optimization reference installed hosted headers compile, link, and execute"
            "$optimization returns-twice control flow and native scalar atomics"
            "$optimization VLA bounds, runtime sizeof, and hosted arena provider"
            "$optimization VLA hosted provider failure traps"
            "$optimization unwind crosses fixed frames and both variadic bridges"
            "$optimization reference threads use isolated CCC TLS definitions"
            "$optimization CCC accesses a reference compiler TLS definition"
        )
        if [[ $target_name == x86_64-linux ]]; then
            TARGET_ORACLE_CASE_PLAN+=(
                "$optimization GCC caller to CCC x87 long-double boundaries and operations"
                "$optimization CCC caller to GCC x87 long-double boundaries"
            )
        fi
    done

    TARGET_ORACLE_CASE_PLAN+=(
        "implicit and explicit normalized target options emit identical objects"
        "predefined target identity matches the reference compiler profile"
    )

    case $target_name in
        x86_64-linux)
            TARGET_ORACLE_CASE_PLAN+=(
                "x87 layout and uninitialized long-double storage remain available"
                "ELF TLS sections, source metadata, bindings, accessor localization, and relocation models"
                "ELF machine, flags, CFI, stack, symbols, relocations, and PIE contract"
            )
            ;;
        aarch64-linux|riscv64-linux)
            TARGET_ORACLE_CASE_PLAN+=(
                "binary128 layout and uninitialized long-double storage remain available"
                "long_double_initialization.c rejects with CCC2343"
                "long_double_conversion.c rejects with CCC2343"
                "long_double_arithmetic.c rejects with CCC2343"
                "long_double_boundary.c rejects with CCC2343"
                "long_double_aggregate_boundary.c rejects with CCC3509"
                "long_double_va_arg.c rejects with CCC2404"
                "ELF TLS sections, source metadata, bindings, accessor localization, and relocation models"
                "ELF machine, flags, CFI, stack, symbols, relocations, and PIE contract"
            )
            ;;
        darwin-arm64)
            TARGET_ORACLE_CASE_PLAN+=(
                "Darwin binary64 long double crosses the Apple ABI"
                "Mach-O TLV sections, relocations, symbol spelling, and accessor localization"
                "Mach-O build version, CFI, symbols, visibility, relocation, and PIE contract"
                "one-step debug links preserve OSO inputs and publish lines, parameters, stack and promoted locals, and TLS metadata through dSYM"
            )
            ;;
    esac

    TARGET_ORACLE_CASE_PLAN+=(
        "debugger stops in the variadic entry and generated call helper with caller frames"
    )
}

target_oracle_expect_case() {
    local case_index=$1
    local observed_name=$2
    local case_number=$((case_index + 1))
    local plan_size=${#TARGET_ORACLE_CASE_PLAN[@]}
    local expected_name

    if (( case_index >= plan_size )); then
        printf 'target oracle case [%02d] unexpected: plan ended before "%s"\n' \
            "$case_number" "$observed_name" >&2
        return 1
    fi

    expected_name=${TARGET_ORACLE_CASE_PLAN[$case_index]}
    if [[ $observed_name != "$expected_name" ]]; then
        printf 'target oracle case [%02d] mismatch: expected "%s"; observed "%s"\n' \
            "$case_number" "$expected_name" "$observed_name" >&2
        return 1
    fi
}

target_oracle_expect_plan_complete() {
    local observed_count=$1
    local plan_size=${#TARGET_ORACLE_CASE_PLAN[@]}
    local next_case_number next_case_name

    if (( observed_count == plan_size )); then
        return 0
    fi
    if (( observed_count < plan_size )); then
        next_case_number=$((observed_count + 1))
        next_case_name=${TARGET_ORACLE_CASE_PLAN[$observed_count]}
        printf 'target oracle stopped after %d cases; missing case [%02d] "%s" (plan has %d cases)\n' \
            "$observed_count" "$next_case_number" "$next_case_name" "$plan_size" >&2
        return 1
    fi

    printf 'target oracle ran %d cases after its %d-case plan ended\n' \
        "$observed_count" "$plan_size" >&2
    return 1
}
