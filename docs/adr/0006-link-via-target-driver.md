# ADR-0006 — Link via a resolved target compiler driver

Status: accepted (2026-07-13)

## Context

CCC produces `.o` files via `cranelift-object` and must turn them into executables, shared objects, and archives. Linking requires CRT startup objects, default libraries, runtime helpers, and platform-specific search paths.

## Decision

Drive executable and shared-library linking through a **resolved target compiler driver** rather than invoking `ld` directly or bundling a linker. `ToolchainResolver` must prove that the driver, sysroot/SDK, CRT objects, libraries, and target match the requested effective configuration. The host `cc` is eligible only for a matching native target.

## Alternatives

- **Invoke `ld` directly.** Requires reimplementing per-platform CRT object selection, default library sets, and search-path logic.
- **Bundle a linker (`lld`/`mold`).** Increases the distribution and still needs platform CRT/library glue.

## Rationale

Inherit the selected target toolchain's CRT objects, default libraries, linker flavor, and search paths while retaining an explicit, inspectable [link plan](../design/driver-cli.md#target-and-link-planning). CCC still selects and verifies compiler-emitted runtime helpers through its manifest.

## Consequences

- Depends on a matching target toolchain and sysroot/SDK being installed or explicitly configured.
- Cross compilation fails clearly when only a host driver is available.
- Static archives use the resolved archiver rather than the compiler driver.

## Revisit if

A self-contained, toolchain-free CCC distribution becomes a requirement.
