# ADR-0008 — Pin Cranelift; upgrade deliberately

Status: accepted (2026-07-13)

## Context

Cranelift (`cranelift-codegen`, `cranelift-frontend`, `cranelift-object`) is the code-generation backend and its API surface is load-bearing for `ccc-abi` and `ccc-codegen`. Cranelift's APIs move quickly across releases.

## Decision

Pin exact versions of the Cranelift crates (and `object`, `target-lexicon`, `gimli`) through exact workspace constraints and the committed lockfile. Upgrade in isolated PRs, gated by the [ABI oracle](../design/testing.md#abi-oracle), backend-capability tests, object inspection, and the execution suite.

## Alternatives

- **Track latest continuously.** Keeps up with fixes/features but risks silent codegen/ABI regressions landing mixed with unrelated work.

## Rationale

The ABI and codegen surface is where subtle backend changes could silently miscompile. Isolating upgrades makes any regression bisectable and attributable.

## Consequences

- Periodic, deliberate upgrade PRs rather than automatic bumps.
- New Cranelift features remain unavailable until a candidate version passes CCC's capability and correctness gates.

## Revisit if

The upgrade gates no longer catch backend regressions proportionately to their cost, or the backend adopts a stability policy that makes exact pinning counterproductive.
