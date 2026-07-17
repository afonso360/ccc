# Linux runner prerequisites

The private Linux x86-64 runner supplies native ABI references, packaging tools,
and hosted corpus dependencies. Provision all requirements together so a runner
cannot pass the Rust build while silently skipping native evidence.

For Ubuntu 24.04, install:

```text
sudo apt-get install gcc clang make tcl-dev zlib1g-dev pkg-config binutils gdb coreutils curl
```

The resulting environment must provide these commands:

```text
curl gcc clang make tclsh pkg-config objdump readelf gdb objcopy timeout
```

Both `pkg-config --exists tcl` and `pkg-config --exists zlib` must succeed. The
workflow records compiler targets and versions plus Make, Tcl, and zlib versions
before running any native ABI or corpus test.

Provisioning is runner administration. The workflow checks and reports the
environment but does not mutate the self-hosted machine with `apt`.
