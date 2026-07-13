# ADR-0001 — CCC-IR middle layer

Status: accepted (2026-07-13)

## Context

Between the typed AST and Cranelift IR (CLIF), a C compiler must desugar the surface language, make ABI decisions, and (optionally) optimize. This work can happen directly during AST→CLIF lowering, or in a dedicated intermediate representation.

## Decision

Introduce **CCC-IR**: a typed, ABI-agnostic mid-level IR in CFG form between the typed AST and Cranelift. Its semantic contract is [CCC-IR invariants](../design/ccc-ir.md).

## Alternatives

- **Lower AST→CLIF directly.** Less code up front, but desugaring, C-level optimization, and any second backend all become entangled with Cranelift specifics.

## Rationale

- One canonical place to desugar the surface language (loops, short-circuits, compound assignment, implicit conversions) — done once, not per backend.
- A backend-independent home for cheap C-aware optimizations (const-fold, DCE, mem-to-reg).
- A stable extension seam: a future second backend (LLVM, an interpreter for constant-eval) or a second frontend can attach here without touching the frontend.

## Consequences

- An extra IR layer to define, build, and maintain.
- A clear contract (`ccc-sema` → CCC-IR) that keeps lower layers free of type inference.

## Revisit if

Measurements show that the layer adds material complexity or compile-time cost without preserving a useful semantic boundary, and equivalent invariants can be enforced directly in every backend without duplication.
