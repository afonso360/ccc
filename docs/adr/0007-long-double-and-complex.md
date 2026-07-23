# ADR-0007 — Preserve target `long double` ABI and fail unsupported complex use

Status: accepted (2026-07-13)

## Context

Platform C ABIs use x87 extended precision, IEEE binary128, or binary64 for `long double`. Cranelift's support varies by representation and version, and arithmetic support alone does not provide platform argument/return conventions. Complex types add arithmetic and ABI rules of their own.

## Decision

- The default configuration always reports and preserves the target's native `long double` size, alignment, macros, object representation, and ABI.
- CCC uses verified backend support, runtime arithmetic, and generated ABI bridges as described in [Conformance](../design/conformance.md#long-double). If an operation or boundary lacks a complete capability, CCC emits a hard diagnostic; it never silently substitutes `double`.
- ABI-changing `long double` overrides such as `-mlong-double-64` are rejected;
  every enabled profile uses its target-native representation coherently.
- When complex support is unavailable, CCC defines `__STDC_NO_COMPLEX__`, feature predicates report false, and semantic use is a hard error. Parsing the type spelling is not a claim of support.

## Alternatives

- **Default `long double` to f64.** Makes common libc/foreign calls ABI-incompatible and lets a warning become silent memory or register corruption.
- **Implement only soft-float arithmetic.** Preserves values internally but does not solve x87 or binary128 ABI boundaries.
- **Reject every declaration containing `long double`.** Prevents otherwise valid header parsing, `sizeof`, and layout inspection even when no unavailable operation is used.
- **Advertise complex syntax without arithmetic/ABI support.** Causes headers and feature tests to select code paths CCC cannot compile safely.

## Consequences

- Some parsed programs fail at a precise semantic use until the selected target has arithmetic and ABI capabilities.
- Target bridge/runtime code becomes part of the tested compiler distribution.
- Compatibility-mode objects can be identified and rejected when mixed incompatibly.

## Revisit if

All supported targets gain complete native backend arithmetic and ABI lowering for their `long double` and complex representations. Native support may replace bridges without changing the public conformance contract.
