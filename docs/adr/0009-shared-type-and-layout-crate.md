# ADR-0009 — Shared type & layout crate (`ccc-types`)

Status: accepted (2026-07-13)

## Context

Three consumers need one answer about C types: semantic analysis computes layout (`sizeof`, bitfields, `offsetof`) during constant evaluation; CCC-IR is typed; `ccc-abi` classifies aggregates from type layout. The workspace assigned types to no crate, which forces either sema's AST-era types to flow into codegen or a second type system inside CCC-IR — two layout engines that must agree.

## Decision

Add **`ccc-types`**: the canonical, interned C type representation (types, qualifiers, tags, bitfield descriptors) and the single layout engine, keyed by the [`EffectiveCompilationConfig`](../design/targets.md#effective-compilation-configuration). `ccc-sema` constructs and interns types; the typed AST, CCC-IR, `ccc-abi`, and `ccc-codegen` reference the same interned types. Layout is computed exactly once per (type, configuration).

## Alternatives

- **Sema-owned types.** Drags all of `ccc-sema` into the dependency set of every backend crate.
- **IR-owned duplicate types.** Requires a sema→IR type mapping plus a second layout engine; divergence between them is exactly the class of silent ABI bug the project exists to prevent.

## Consequences

- One more small foundational crate; it sits above `ccc-target` and below sema/IR/ABI/codegen.
- The sema→IR boundary passes type IDs, not converted type structures.

## Revisit if

The type representation needs sema-only or backend-only variants that make one shared table more costly than a defined conversion.
