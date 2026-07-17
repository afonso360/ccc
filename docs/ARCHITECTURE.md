# CCC — Architecture

**CCC** is the **C**ranelift **C** **C**ompiler: a Rust-based C compiler using [Cranelift](https://cranelift.dev/) as its code-generation backend. It targets x86-64, AArch64, and RISC-V64, and aims to be a drop-in replacement for `clang`/`gcc` — usable inside real build systems (`make`, CMake, autotools).

CCC targets **pragmatic C11 plus a GNU compatibility subset** — a deliberate contract, not full ISO strict-conformance. Known deviations (notably `long double` and `_Complex`) are enumerated and tracked, never left implicit. The goal is a compiler that builds real-world C, matches GCC/Clang observable behavior, and states precisely where it does not.

This document is the high-level introduction. Each design area is detailed in its own document under [`docs/design/`](design/); each significant decision is an ADR under [`docs/adr/`](adr/).

Status: design. Last updated 2026-07-15.

---

## Scope reality check

"Drop-in replacement, C11" is three separate commitments, and the third is larger than it looks:

- **C11 the language** — fully specified, tractable (with the enumerated conformance gaps in [Conformance policy](design/conformance.md)).
- **CLI + toolchain compatibility** — the GCC/Clang flag surface, plus resolving and driving the matching target assembler, linker, archiver, sysroot, and runtime.
- **Compiling _real_ code** — the moment you `#include <stdio.h>`, you are compiling glibc/musl/Darwin headers, saturated with GNU extensions: `__attribute__`, `__builtin_*`, statement expressions, `typeof`, inline asm, `__int128`. **Pure C11 is not enough to be a drop-in replacement.**

A truly drop-in compiler is a multi-person-year effort. The architecture keeps every layer independently testable and makes unsupported capabilities fail explicitly instead of silently changing C or platform semantics.

## Guiding principles

1. **End-to-end executability.** Every advertised language slice flows through preprocessing, parsing, semantics, ABI planning, object generation, target linking, and execution tests. No isolated phase is considered supported without its downstream path.
2. **Stable IR boundaries.** Each stage consumes and produces an explicit, testable data structure. This is what makes the compiler extensible — the backend, an optimizer, or an additional frontend can be swapped without a rewrite.
3. **Targets and options are data.** Target defaults plus language/ABI/codegen/toolchain options form one immutable effective configuration used by every phase; sizes, macros, ABI, and linker behavior are never independently hard-coded.
4. **Differential correctness.** "Correct" means "matches GCC/Clang observable behavior on the same _well-defined_ input," tested mechanically. Programs exercising undefined or unspecified behavior are excluded from differential comparison by construction.

## Pipeline at a glance

C distinguishes **preprocessing tokens** (pp-tokens) from the **parser tokens** the grammar consumes; macro expansion operates on the former, and conversion to the latter is translation phase 7. Both are explicit stages:

```
source bytes
   → driver (CLI, phase orchestration)
   → pp-token lexer  ─┐
   → preprocessor    ─┴─ macro expansion, #include, #if, #pragma
   → token conversion ─┐
   → parser           ─┴─ recursive descent → untyped AST
   → semantic analysis → typed AST (every implicit conversion explicit)
   → IR lowering → CCC-IR (typed, ABI-agnostic, CFG)
   → ABI planning → per-target struct/sret/varargs plans and bridge requirements
   → codegen → Cranelift IR (CLIF)
   → cranelift-object + CCC DWARF emission → .o
   → resolved target assembler / linker / archiver → exe / shared library / archive
```

The fully annotated pipeline — which crate owns each stage, the component breakdown, and the workspace layout — is in [Pipeline & crates](design/pipeline-and-crates.md).

## Design documents

| Document                                                      | Covers                                                                                                  |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| [Targets & non-goals](design/targets.md)                      | Exact target triples, object formats, ABIs, and what's explicitly out of scope                          |
| [Conformance policy](design/conformance.md)                   | `long double`, `_Complex`, effective implementation-defined behavior, GNU capability registry           |
| [Frontend capabilities](design/frontend-capabilities.md)      | Syntax recognition, GNU declarations, capability states, and hosted-header phase certification          |
| [C11 and GNU semantics](design/core-c11-and-gnu-semantics.md) | Activation contract for selected C11 constructs, GNU expressions, wide integers, builtins, and assembly |
| [ABI & variadic functions](design/abi-and-varargs.md)         | ABI plans, aggregate boundaries, variadic call bridges, `va_list`, and target shims                     |
| [Pipeline & crates](design/pipeline-and-crates.md)            | Annotated pipeline, per-component design, workspace/crate layout                                        |
| [CCC-IR invariants](design/ccc-ir.md)                         | Places vs values, aggregates, bitfields, volatile, atomics; sema guarantees                             |
| [Resource directory](design/resource-dir.md)                  | Shipped builtin headers, include search, runtime helper strategy                                        |
| [Driver & CLI](design/driver-cli.md)                          | Flag surface, unknown-flag policy, observability dumps                                                  |
| [Diagnostics](design/diagnostics.md)                          | Spans, macro/include backtraces, parser recovery, warning control                                       |
| [Cranelift risk register](design/cranelift-risks.md)          | The hard backend problems (ABI, varargs, `long double`, inline asm, TLS, …)                             |
| [Testing strategy](design/testing.md)                         | Snapshot/execution/differential tiers, the ABI oracle, corpus licensing                                 |
| [Toolchain & policy](design/toolchain.md)                     | Dependencies, Cranelift pinning, MSRV, lint/dep policy                                                  |

Project planning is documented separately in [ROADMAP.md]; it is not part of
the feature design.

## Decision records

The significant, cross-cutting decisions live as ADRs in [`docs/adr/`](adr/):

| ADR                                                              | Decision                                                                                          |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| [0001](adr/0001-ccc-ir-middle-layer.md)                          | Adopt a CCC-IR middle layer (don't lower AST→CLIF directly)                                       |
| [0002](adr/0002-syntax-owned-typedef-classification.md)          | Syntax owns a shared typedef-classification event model                                           |
| [0003](adr/0003-first-target-triple.md)                          | First target triple: `x86_64-unknown-linux-gnu`                                                   |
| [0004](adr/0004-recursive-descent-parser.md)                     | Hand-written recursive-descent parser                                                             |
| [0005](adr/0005-preprocessor-owns-pp-token-lexing.md)            | Preprocessor owns pp-token lexing                                                                 |
| [0006](adr/0006-link-via-target-driver.md)                       | Link via a resolved target compiler driver                                                        |
| [0007](adr/0007-long-double-and-complex.md)                      | Preserve target `long double` ABI; reject unsupported operations, explicit f64 compatibility mode |
| [0008](adr/0008-pin-cranelift.md)                                | Pin Cranelift; upgrade deliberately                                                               |
| [0009](adr/0009-shared-type-and-layout-crate.md)                 | Shared `ccc-types` crate: one canonical type representation and layout engine                     |
| [0010](adr/0010-generate-abi-bridges-as-assembly.md)             | Generate ABI bridges as auditable target assembly                                                 |
| [0011](adr/0011-arena-backed-runtime-sized-automatic-storage.md) | Back runtime-sized automatic objects with a scoped arena; keep native-stack builtins gated        |
