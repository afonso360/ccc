# Toolchain, dependencies, and project policy

## Rust and library dependencies

The workspace uses exact, mutually compatible versions of `cranelift-codegen`, `cranelift-frontend`, `cranelift-object`, `object`, `target-lexicon`, and `gimli`. Workspace dependency declarations use exact constraints for the Cranelift family, and the committed `Cargo.lock` is the authoritative resolved-version record; documentation does not duplicate a version number that could drift.

Cranelift upgrades occur in isolated changes. Each upgrade records capability changes for variadics, f16/f128, atomic operations, memory flags, dynamic stack allocation, TLS, object relocations, and debug information, then runs the ABI oracle, object inspection, CLIF verification, execution suite, and backend-specific regression corpus. A newly present API is not enabled until its emitted behavior passes those tests.

## Target toolchain resolution

`ToolchainResolver` constructs the [`ToolchainSpec`](targets.md#effective-compilation-configuration) used for preprocessing, assembling, linking, runtime selection, and execution. Resolution order is deterministic:

1. explicit command-line paths/options such as `--gcc-toolchain`, `--sysroot`, `-isysroot`, linker selection, SDK, and deployment target;
2. target-specific CCC configuration or environment entries;
3. a native host driver only when its reported target matches the requested target;
4. otherwise, a hard error explaining which target tool is missing.

There is no cross-target fallback to an unverified host `cc`.

Resolution is phase-scoped: preprocessing and compilation require the sysroot/SDK and system include tree; assembling requires the assembler; linking requires the linker driver, CRT objects, and runtime libraries. A missing component is an error only when a selected phase needs it — `ccc -E` or `ccc -c` never fails for want of a linker.

Resolution is requirements-scoped in the implementation: `-nostdinc` with no
other system-dependent action does not probe for a system include tree, and a
preprocess-only action never resolves a linker.

The resolver probes and fingerprints the selected tools using machine-readable or stable driver queries where available: reported target, sysroot, search directories, startup objects, runtime-library paths, assembler/linker identity, PIE default, multilib selection, and builtin/system include directories. Probe results are cached by executable identity, version, target options, sysroot/SDK, and relevant environment. Missing or contradictory results are errors, not guessed paths.

For GCC- and Clang-compatible drivers, preprocessing resolution separately
probes the reported target, sysroot, and the delimited include-search listing
from a no-code preprocessing invocation. The include-list parser is tested
against recorded GCC and Clang output. A fingerprint covers the canonical
driver path, version output, target options, sysroot, relevant environment,
and normalized probe results. A Darwin development host cannot supply the
Linux GNU system-header gate by fallback; local deterministic tests use
`-nostdinc`, recorded probes, or a fake sysroot.

Darwin requires a compatible Apple SDK, deployment target, and Apple-capable linker. Linux GNU and musl configurations resolve distinct CRTs, dynamic loaders, libraries, and header trees. `ccc-link` invokes the resolved target compiler driver for executable/shared linking, the resolved assembler for generated bridge/assembly files, and the resolved `ar`/`ranlib` pair (or a verified in-process archive writer) for static archives.

## Runtime helper manifest

Every compiler-emitted helper has a manifest entry containing symbol, exact C/ABI signature, provider preference, target availability, and a conformance test. Providers may be compiler-rt, libgcc, libatomic, libc, or a versioned CCC runtime shim. The link plan names the selected provider; it never assumes the target driver happens to supply a helper with the desired ABI.

CCC runtime shims use a versioned symbol namespace except where an external ABI mandates a standard helper name. Runtime objects are selected by target and effective ABI options, including long-double mode, and incompatible CCC objects are diagnosed.

## Rust project policy

- The MSRV is identical in `rust-toolchain.toml` and workspace `rust-version` fields.
- `rustfmt` and `clippy` run in CI; lint policy is pinned with the toolchain so a compiler update cannot break unrelated changes silently.
- Dependency updates are batched, security updates are expedited, and generated lockfile changes are reviewed with license and advisory checks.
- Unsafe code is isolated at FFI/object/backend boundaries and documents its invariants.
