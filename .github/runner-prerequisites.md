# Linux runner prerequisites

The required Linux jobs use GitHub's Ubuntu 24.04 x86-64 runner. The workflow
installs native ABI references, packaging tools, and hosted corpus dependencies
together so a runner cannot pass the Rust build while silently skipping native
evidence.

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

The workflow installs this package set for each required run, then checks and
records the resulting command, compiler, Tcl, and zlib identities. The manually
dispatched Csmith job remains on the private runner and verifies its own larger
prerequisite set independently.

## Cross-Linux target runners

The Ubuntu-hosted Linux x86-64 runner supplies Docker. Matching GNU cross
compilers, sysroots, binutils, user-mode QEMU, and `gdb-multiarch` are installed
inside the image defined by `target-oracle-linux.Dockerfile`, not on the runner.
That image pins its base digest, dated Debian snapshot, package versions, and
post-install identities. Its command set is:

```text
aarch64-linux-gnu-gcc aarch64-linux-gnu-objdump aarch64-linux-gnu-readelf qemu-aarch64
riscv64-linux-gnu-gcc riscv64-linux-gnu-objdump riscv64-linux-gnu-readelf qemu-riscv64
gdb-multiarch
```

The AArch64 compiler target must be `aarch64-linux-gnu`. The RISC-V compiler
must target RV64GC with the LP64D ABI. The workflow records the resolved
sysroots and passes them explicitly to QEMU; it must not use an unrelated host
root. Required jobs fail when a command, sysroot, dynamic loader, gdbstub, or
applicable test is missing. Each target runs in a separate container, isolating
its QEMU gdbstub ports and package state.

## Darwin arm64 target runner

The native Darwin job requires an Apple-silicon host with `xcrun`, Apple Clang,
the macOS SDK, `nmedit`, LLDB, `otool`, `nm`, and `dwarfdump`. Mach-O generated
symbols are localized with the Command Line Tools' native `nmedit`; LLVM
`objcopy` is not part of this profile. The job records the Command Line Tools
build, SDK version, Apple Clang version and target, linker and `nmedit`
identities, and deployment target. The recorded identities are compared with
the Darwin evidence manifest before tests run. A non-arm64 host or a missing
native tool is a hard failure for the required job.
