# Execution tests

The harness invokes the built `ccc` binary against every fixture in this
directory under `-O0`, `-O2`, and `-Oz`. Native x86-64 Linux, AArch64 Linux,
RISC-V Linux, and arm64 Darwin hosts each check their own object format, link
the applicable fixtures, execute them, and verify their exit status and output.
Fixtures use behavior defined by C11 or by the selected GNU-compatible target
configuration; explicitly architecture-specific cases are marked in the case
table instead of excluding an entire host suite. They do not compare padding
bytes, depend on unspecified evaluation order, or access inactive union
members. The empty translation unit is object-only because a normal hosted C
runtime requires `main` when linking an executable.
