# Linux runner prerequisites

The private Linux x86-64 runner supplies native ABI references, packaging tools,
and hosted corpus dependencies. Provision all requirements together so a runner
cannot pass the Rust build while silently skipping native evidence.

For Ubuntu 24.04, install:

```text
sudo apt-get install gcc g++ clang cmake make m4 tcl-dev zlib1g-dev pkg-config binutils gdb coreutils curl openssl
```

The resulting environment must provide these commands:

```text
curl gcc clang make tclsh pkg-config objdump readelf gdb objcopy timeout
```

Both `pkg-config --exists tcl` and `pkg-config --exists zlib` must succeed. The
workflow records compiler targets and versions plus Make, Tcl, and zlib versions
before running any native ABI or corpus test.

The manually dispatched Csmith workflow additionally requires `g++`, `cmake`,
`m4`, `objcopy`, `openssl`, and `tar`. It builds a cryptographically verified
Csmith source pin inside the workflow artifact directory; Csmith is not
installed globally on the runner.

Provisioning is runner administration. The workflow checks and reports the
environment but does not mutate the self-hosted machine with `apt`.

## Cross-Linux target runners

Required AArch64 and RISC-V Linux jobs must provide matching GNU cross
compilers, sysroots, binutils, user-mode QEMU, and `gdb-multiarch`. The command
set is:

```text
aarch64-linux-gnu-gcc aarch64-linux-gnu-objdump aarch64-linux-gnu-readelf qemu-aarch64
riscv64-linux-gnu-gcc riscv64-linux-gnu-objdump riscv64-linux-gnu-readelf qemu-riscv64
gdb-multiarch
```

The AArch64 compiler target must be `aarch64-linux-gnu`. The RISC-V compiler
must target RV64GC with the LP64D ABI. The workflow records the resolved
sysroots and passes them explicitly to QEMU; it must not use an unrelated host
root. Required jobs fail when a command, sysroot, dynamic loader, gdbstub, or
applicable test is missing.

## Darwin arm64 target runner

The native Darwin job requires an Apple-silicon host with `xcrun`, Apple Clang,
the macOS SDK, LLDB, `otool`, `nm`, and `dwarfdump`. It records the Command Line
Tools or Xcode build, SDK version, Apple Clang version and target, linker
version, and deployment target. The recorded identities are compared with the
Darwin evidence manifest before tests run. A non-arm64 host or a missing native
tool is a hard failure for the required job.
