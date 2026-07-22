#!/usr/bin/env bash

# This file is sourced by run.sh. Keep the generator surface fixed so a seed
# identifies the same source program on every run of the pinned Csmith build.

csmith_version=2.4.0
csmith_revision=0cdc710315cfee9035e22ef4363ca479270d1934f
csmith_archive_name=csmith-0cdc710315cfee9035e22ef4363ca479270d1934f.tar.gz
csmith_archive_origin=https://codeload.github.com/csmith-project/csmith/tar.gz/0cdc710315cfee9035e22ef4363ca479270d1934f
csmith_archive_bytes=330825
csmith_archive_sha256=f85081c3cc817770c1664b5d4457d1b91120e26daff86e24aa3e7fb91941c611
csmith_archive_sha3_256=35f8dcc176f8a99af6672af270891bf65b070a19036713412974d8038ec9141e

generator_options=(
  --no-argc
  --no-float
  --no-packed-struct
  --no-unions
  --no-bitfields
  --no-builtins
  --no-dangling-global-pointers
  --no-return-dead-pointer
  --strict-volatile-rule
  --match-exact-qualifiers
  --safe-math
  --max-funcs 5
  --max-block-depth 4
  --max-block-size 4
  --max-expr-complexity 8
  --max-array-dim 2
  --max-array-len-per-dim 5
)

reference_optimizations=(-O0 -O2)

default_cases=100
default_start_seed=1
default_build_jobs=2
default_maximum_attempt_multiplier=10
default_generator_timeout=30
default_compile_timeout=60
default_execution_timeout=10

write_csmith_manifest() {
  cat <<EOF
format_version = 2
name = "csmith"
version = "$csmith_version"
revision = "$csmith_revision"
distribution = "official-commit-source"
origin = "$csmith_archive_origin"
archive = "$csmith_archive_name"
archive_bytes = $csmith_archive_bytes
archive_sha256 = "$csmith_archive_sha256"
archive_sha3_256 = "$csmith_archive_sha3_256"
license = "BSD-3-Clause"
upstream_license_file = "COPYING"
fetched_not_vendored = true

target = "x86_64-unknown-linux-gnu"
language_mode = "c11"
profile = "profile.sh"
generator_options = [
EOF
  printf '    "%s",\n' "${generator_options[@]}"
  cat <<EOF
]
reference_compilers = ["gcc", "clang"]
reference_optimizations = ["-O0", "-O2"]
target_compiler = "ccc"
output_oracle = "reference-consensus"
eligibility = "gcc-and-clang-strict-c11"
inconclusive_policy = "replace-joint-reference-timeout"
link_driver = "gcc"
link_libraries = ["m"]

[defaults]
cases = $default_cases
start_seed = $default_start_seed
build_jobs = $default_build_jobs
maximum_attempt_multiplier = $default_maximum_attempt_multiplier
generator_timeout_seconds = $default_generator_timeout
compile_timeout_seconds = $default_compile_timeout
execution_timeout_seconds = $default_execution_timeout

[retained_artifacts]
run = [
    "run-config.txt",
    "run-summary.txt",
    "summary.tsv",
    "tool-identities",
]
case = [
    "program.c",
    "commands.txt",
    "result.txt",
    "*.status",
    "*.stdout",
    "*.stderr",
]

[target_applicability."x86_64-unknown-linux-gnu"]
status = "applicable"
evidence_kind = "execution"
runner = "run.sh"
reason = "The runner validates an x86-64 Linux GNU host and compares CCC execution with matching native GCC and Clang reference executions."

[target_applicability."aarch64-unknown-linux-gnu"]
status = "inapplicable"
reason = "The consensus runner has no matched AArch64 GCC and Clang cross-reference matrix or QEMU execution path."

[target_applicability."riscv64-unknown-linux-gnu"]
status = "inapplicable"
reason = "The consensus runner has no matched RISC-V GCC and Clang cross-reference matrix or QEMU execution path."

[target_applicability."aarch64-apple-darwin"]
status = "inapplicable"
reason = "The runner requires a Linux GNU target and has no Darwin reference-consensus or native execution profile."
EOF
}
