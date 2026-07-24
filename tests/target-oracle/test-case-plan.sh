#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
source "$script_dir/case-plan.sh"

fail() {
    echo "case-plan regression failed: $*" >&2
    exit 1
}

plan_has_case() {
    local wanted=$1
    local case_name
    for case_name in "${TARGET_ORACLE_CASE_PLAN[@]}"; do
        if [[ $case_name == "$wanted" ]]; then
            return 0
        fi
    done
    return 1
}

assert_plan_shape() {
    local target_name=$1
    local plan_size case_index other_index

    target_oracle_build_case_plan "$target_name"
    plan_size=${#TARGET_ORACLE_CASE_PLAN[@]}
    (( plan_size > 0 )) || fail "$target_name produced an empty plan"

    for ((case_index = 0; case_index < plan_size; case_index++)); do
        [[ -n ${TARGET_ORACLE_CASE_PLAN[$case_index]} ]] ||
            fail "$target_name has an empty case at index $case_index"
        target_oracle_expect_case \
            "$case_index" "${TARGET_ORACLE_CASE_PLAN[$case_index]}"
        for ((other_index = case_index + 1; other_index < plan_size; other_index++)); do
            [[ ${TARGET_ORACLE_CASE_PLAN[$case_index]} != \
                "${TARGET_ORACLE_CASE_PLAN[$other_index]}" ]] ||
                fail "$target_name duplicates case ${TARGET_ORACLE_CASE_PLAN[$case_index]}"
        done
    done
    target_oracle_expect_plan_complete "$plan_size"

    plan_has_case "-O0 reference caller to CCC fixed ABI" ||
        fail "$target_name is missing its first optimization case"
    plan_has_case "-O2 CCC accesses a reference compiler TLS definition" ||
        fail "$target_name is missing its last common optimization case"
    [[ ${TARGET_ORACLE_CASE_PLAN[$((plan_size - 1))]} == \
        "debugger stops in the variadic entry and generated call helper with caller frames" ]] ||
        fail "$target_name does not end with its common debugger case"

    case $target_name in
        x86_64-linux)
            plan_has_case "-O0 GCC caller to CCC x87 long-double boundaries and operations" ||
                fail "$target_name is missing x87 ABI coverage"
            plan_has_case "x87 layout and uninitialized long-double storage remain available" ||
                fail "$target_name is missing x87 storage coverage"
            ;;
        aarch64-linux|riscv64-linux)
            plan_has_case "binary128 layout and uninitialized long-double storage remain available" ||
                fail "$target_name is missing binary128 storage coverage"
            plan_has_case "long_double_va_arg.c rejects with CCC2404" ||
                fail "$target_name is missing binary128 rejection coverage"
            ;;
        darwin-arm64)
            plan_has_case "Darwin binary64 long double crosses the Apple ABI" ||
                fail "$target_name is missing Darwin long-double coverage"
            plan_has_case "one-step debug links preserve OSO inputs and publish lines, parameters, stack and promoted locals, and TLS metadata through dSYM" ||
                fail "$target_name is missing dSYM source-debug coverage"
            ;;
    esac
}

for target_name in x86_64-linux aarch64-linux riscv64-linux darwin-arm64; do
    assert_plan_shape "$target_name"
done

target_oracle_build_case_plan x86_64-linux
if diagnostic=$(target_oracle_expect_case \
        0 "${TARGET_ORACLE_CASE_PLAN[1]}" 2>&1); then
    fail "skipping the first case did not fail"
fi
[[ $diagnostic == *'case [01] mismatch:'* ]] ||
    fail "skip diagnostic has no useful case index: $diagnostic"
[[ $diagnostic == *"expected \"${TARGET_ORACLE_CASE_PLAN[0]}\""* ]] ||
    fail "skip diagnostic has no expected case name: $diagnostic"
[[ $diagnostic == *"observed \"${TARGET_ORACLE_CASE_PLAN[1]}\""* ]] ||
    fail "skip diagnostic has no observed case name: $diagnostic"

plan_size=${#TARGET_ORACLE_CASE_PLAN[@]}
if diagnostic=$(target_oracle_expect_plan_complete "$((plan_size - 1))" 2>&1); then
    fail "omitting the final case did not fail"
fi
[[ $diagnostic == *"missing case [$plan_size]"* ]] ||
    fail "incomplete-plan diagnostic has no useful case index: $diagnostic"
[[ $diagnostic == *"${TARGET_ORACLE_CASE_PLAN[$((plan_size - 1))]}"* ]] ||
    fail "incomplete-plan diagnostic has no missing case name: $diagnostic"

if diagnostic=$(target_oracle_expect_case "$plan_size" "unplanned case" 2>&1); then
    fail "an extra case did not fail"
fi
[[ $diagnostic == *"case [$((plan_size + 1))] unexpected:"* ]] ||
    fail "extra-case diagnostic has no useful case index: $diagnostic"
[[ $diagnostic == *'"unplanned case"'* ]] ||
    fail "extra-case diagnostic has no observed case name: $diagnostic"

echo "Target-oracle case plans validated for all targets"
