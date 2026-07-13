# ADR-0003 — First target triple: `x86_64-unknown-linux-gnu`

Status: accepted (2026-07-13)

## Context

The compiler needs one primary reference target with a concrete ABI, object format, libc, toolchain, and predefined-macro contract. The full [support matrix](../design/targets.md) is additive on top of it.

## Decision

The primary reference triple is **`x86_64-unknown-linux-gnu`** (ELF, System V AMD64 psABI, LP64, glibc). Every additional target must satisfy the capability manifest in [Targets](../design/targets.md).

## Alternatives

- **Darwin arm64 first** — better `long double` conformance (`long double == double`), but smaller CI ubiquity and a more idiosyncratic ABI/object format for early work.
- **RISC-V first** — least mature tooling ecosystem for a bootstrap.

## Rationale

- Best reference material: cg_clif, the SysV psABI, and abundant existing tooling.
- Ubiquitous CI and developer machines.
- Widest test-corpus coverage.

## Consequences

- Native f80 `long double` makes the required runtime and x87 ABI-bridge capability unavoidable on the reference target; the default contract remains ABI-correct and unsupported operations fail explicitly.

## Revisit if

The primary reference target ceases to represent the deployment or contributor environment, or another supported target offers materially better risk coverage. Additional targets remain additive and must not weaken existing target contracts.
