# ADR-0004 — Hand-written recursive-descent parser

Status: accepted (2026-07-12)

## Context

The parser is `ccc-syntax`'s core. C is context-sensitive (see [ADR-0002](0002-syntax-owned-typedef-classification.md)) and diagnostics quality is a primary product goal ([Diagnostics](../design/diagnostics.md)).

## Decision

Write the parser by hand as **recursive descent**.

## Alternatives

- **Parser generator (LALR/PEG).** Less hand-written code, but weaker error messages and awkward handling of C's context sensitivity and ambiguities.

## Rationale

- Best-in-class diagnostics and error recovery.
- Natural fit for the typedef/name-classification interplay.
- Industry precedent: both Clang and GCC hand-write their C parsers.

## Consequences

- More hand-written parsing code to maintain.

## Revisit if

Maintenance cost ever outweighs the diagnostic benefit (considered unlikely for C).
