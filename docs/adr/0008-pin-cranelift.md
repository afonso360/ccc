# ADR-0008 — Track Cranelift main with a reproducible lock

Status: accepted (revised 2026-07-24)

## Context

Cranelift (`cranelift-codegen`, `cranelift-frontend`, `cranelift-module`,
`cranelift-object`, and their internal crates) is the code-generation backend.
Its API and emitted behavior are load-bearing for ABI planning, object
packaging, unwind information, debug information, and every supported target.
CCC also needs backend capabilities that are developed between Cranelift
releases, including the current inlining interfaces.

## Decision

Declare the complete Cranelift family as Git dependencies on the Wasmtime
repository's `main` branch. The committed `Cargo.lock` pins one exact commit,
so normal and release builds never follow a moving branch implicitly.

Every Cranelift package must resolve from that same commit. The current ABI
configuration key records the audited commit as backend provenance, and a test
rejects a mixed registry/Git or mixed-revision lockfile. Lockfile refreshes are
isolated changes gated by the
[ABI oracle](../design/testing.md#abi-oracle), backend-capability tests, object
inspection, debugger checks, and the execution suite.

CCC owns System V call-frame emission for now. Cranelift's separate object
unwind feature stays disabled so the two implementations cannot emit duplicate
or conflicting unwind sections.

## Alternatives

- **Use exact crates.io releases.** This maximizes release stability but delays
  access to backend facilities CCC is actively integrating.
- **Resolve `main` without committing a lockfile.** This follows upstream most
  quickly but makes ordinary builds irreproducible and regressions difficult to
  bisect.
- **Enable both object unwind emitters.** This duplicates ownership of the same
  sections and risks inconsistent call-frame data.

## Rationale

The branch supplies the backend surface CCC intends to consume, while the
lockfile preserves reproducibility and bisectability. Explicit provenance and a
single-source test make dependency drift visible. Keeping one owner for unwind
information avoids performing the same backend work twice.

## Consequences

- Periodic, deliberate lockfile refreshes rather than automatic changes to
  normal builds.
- A scheduled job tests upstream head without committing its candidate lockfile.
- Routine refreshes may require adapter changes when upstream APIs move.
- Newly present Cranelift features remain unavailable until their emitted
  behavior passes CCC's capability and correctness gates.

## Revisit if

Cranelift publishes a stable interface that includes the facilities CCC needs,
or CCC transfers ownership of unwind/object emission to Cranelift after
equivalent cross-target evidence.
