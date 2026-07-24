# CCC

CCC is a C compiler written in Rust. It implements a practical subset of C11
and GNU C with its own preprocessor, frontend, intermediate representation, and
ABI handling. Machine-code generation uses
[Cranelift](https://cranelift.dev/).

Unsupported constructs are diagnosed rather than silently compiled with
different semantics.

## Build and use

CCC requires Rust 1.96 and a GCC- or Clang-compatible toolchain for the target
platform. The external toolchain supplies system headers, the assembler,
linker, and platform runtime libraries.

```sh
cargo build --release
./target/release/ccc hello.c -o hello
./hello
```

To compile without linking or inspect preprocessed output:

```sh
./target/release/ccc -c hello.c
./target/release/ccc -E hello.c
```

The default language mode is `gnu11`. Use `-std=c11` for strict C11
preprocessing and language rules.

## Current status

Supported targets:

| Target | Notes |
| --- | --- |
| `x86_64-unknown-linux-gnu` | Most complete target; includes x87 `long double`, `__int128`, and ELF thread-local storage |
| `aarch64-unknown-linux-gnu` | AAPCS64 with ELF thread-local storage; binary128 `long double` layout is available, but value operations are not |
| `riscv64-unknown-linux-gnu` | RV64GC/LP64D with ELF thread-local storage; binary128 `long double` layout is available, but value operations are not |
| `aarch64-apple-darwin` | Apple arm64 ABI; `long double` uses binary64 and thread-local storage uses the platform TLV ABI |

Currently supported:

- C preprocessing, including macros, includes, conditional directives,
  pragmas, and Make dependency output.
- Integer, pointer, `float`, and `double` operations; arrays, structures,
  unions, enums, bit-fields, initializers, and flexible array members.
- Fixed and variadic function calls, aggregates passed by value, function
  pointers, and `setjmp`/`longjmp`-style control flow.
- Compound literals, `_Generic`, `_Static_assert`, `_Alignas`, `_Noreturn`,
  variably modified types, runtime `sizeof`, and hosted automatic VLAs.
- Selected GNU extensions, including statement expressions, computed goto,
  declaration assembly labels, attributes, builtins, and certified x86-64
  inline-assembly forms.
- Naturally aligned 1-, 2-, 4-, and 8-byte integer and pointer atomics.
- Ordered source, assembly, object, archive, and library inputs; PIC and PIE;
  shared libraries; static linking; response files; and common build-system
  queries.
- DWARF debug information and Darwin `.dSYM` generation.

Not currently supported:

- `_Complex`, `_Imaginary`, or value operations on `_Float16`.
- Binary128 `long double` arithmetic or ABI transport on AArch64 and RISC-V
  Linux.
- Aggregate, floating-point, 128-bit, or known-misaligned atomics.
- General GNU `typeof`, arbitrary GNU attributes, arbitrary inline assembly,
  or `asm goto`.
- C++, Objective-C, Windows, 32-bit or big-endian targets, LTO, sanitizers, or
  profiling instrumentation.
- Compiler-generated assembly output through `-S`.

The detailed contracts are documented in
[Architecture](docs/ARCHITECTURE.md),
[Conformance](docs/design/conformance.md),
[Frontend capabilities](docs/design/frontend-capabilities.md), and
[Targets](docs/design/targets.md).

## Testing

Run the Rust test suite locally with:

```sh
cargo test --workspace --all-targets
```

Hosted CI runs the Rust test suite natively on x86-64 Linux, AArch64 Linux, and
AArch64 macOS. The RISC-V Rust tests are cross-compiled and run under QEMU.
A separate x86-64 Linux job builds and runs the bounded SQLite, Lua, bzip2,
zlib, Redis, and zstd corpus profiles, the codegen microbenchmark matrix, and
the C-Ray correctness benchmark. Additional AArch64 Linux, RISC-V Linux, and
arm64 Darwin jobs run bzip2's complete target-specific corpus contract.

The complete local commands and target-specific prerequisites are in
[Test commands and prerequisites](docs/testing.md).

## License

CCC is licensed under [Apache-2.0 WITH LLVM-exception](LICENSE).
