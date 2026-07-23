# Roadmap

This document is forward-looking. The current supported surface and its exact
failure boundaries live in
[Frontend capabilities](design/frontend-capabilities.md),
[Targets and non-goals](design/targets.md), and
[Testing and correctness](design/testing.md). Work is complete only when those
contracts, the capability registry, predefined macros, diagnostics, and
cross-target evidence agree.

The ordering below is intentional. CCC should first consume the backend version
whose APIs it intends to use, then add whole-translation-unit optimization on
top of that stable integration point.

## First: track Cranelift `main` directly

Replace the crates.io version constraints for `cranelift-codegen`,
`cranelift-frontend`, `cranelift-module`, and `cranelift-object` with Git
dependencies on the
[Wasmtime repository's `main` branch](https://github.com/bytecodealliance/wasmtime/tree/main/cranelift).
The committed `Cargo.lock` must still pin one exact Git revision so ordinary
and release builds remain reproducible.

The change must include all of the following:

- Supersede the exact-release-source decision in
  [ADR-0008](adr/0008-pin-cranelift.md) while retaining its isolated-update and
  correctness-gate requirements. Tracking the branch changes the source of
  updates, not the requirement for bisectable lockfile commits.
- Resolve every Cranelift crate, including transitive Cranelift crates, from one
  Wasmtime revision. Reject mixed registry/Git or mixed-revision graphs in CI.
- Replace the backend version text embedded in the ABI configuration key with
  one audited backend-provenance value that matches the locked Git revision.
  Keep that value in one place rather than repeating it in code and documents.
- Put the small amount of unstable upstream API use behind a codegen adapter so
  routine upstream changes do not spread through ABI planning, object
  packaging, DWARF emission, and tests.
- Audit changes to signatures, legalizations, atomics, object relocations,
  unwind information, debug value locations, and target flags before accepting
  each lockfile update. A newly available API is not an enabled CCC capability
  by itself.
- Add a scheduled lockfile-refresh job that tests the current upstream head and
  reports breakage without silently changing normal builds.
- Document how to update, bisect, and temporarily roll back the locked
  revision.

This work is complete when `cargo tree` shows a single Cranelift Git revision
and the workspace tests, all target oracles, debugger checks, ABI cross-links,
adapter regressions, Csmith profiles, and real-code corpus gates pass with
`--locked`.

## Second: enable Cranelift inlining

Use Cranelift's
[`Context::inline`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/context.rs)
and
[`Inline` policy interface](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/inline.rs).
Cranelift provides the transformation but deliberately leaves call-graph
knowledge and heuristics to its user, so CCC must supply the policy and the
proof that each selected call is safe.

### Code-generation integration

- Lower all function definitions to CLIF before compiling any one function.
  Build a deterministic map from a caller's direct `FuncRef` to its
  translation-unit-local definition.
- Legalize and verify every candidate callee before returning it from the
  `Inline` implementation. Require the callee body to have exactly the
  signature referenced by the call site and no remaining `global_value`
  instructions. Keep legalized bodies local to one target/configuration
  invocation; never reuse them across ISA or codegen settings.
- Run inlining before the caller's normal Cranelift optimization and machine
  lowering, then let Cranelift simplify the resulting CFG and values.
- Keep generated ABI bridges, imported functions, indirect calls, patchable
  calls, weak definitions, interposable external definitions, and incompatible
  lowered signatures out of the initial candidate set.
- Preserve the original out-of-line definition whenever its symbol or address
  remains observable.

### Policy and language semantics

- Start heuristic inlining at `-O2` and `-O3`. Use deterministic instruction,
  block, depth, and translation-unit growth budgets; prevent direct and mutual
  recursion from exhausting the compiler. Admit `-Os` and `-Oz` candidates
  only when the measured result is smaller.
- Promote `always_inline` and `noinline` from behavior-compatible attributes to
  exact function properties with diagnostics. Treat the C `inline` specifier
  as a hint, not as permission to ignore its separate C11 definition and
  linkage rules.
- Exclude returns-twice paths, generated bridge bodies, and other exceptional
  frame contracts until focused tests prove that cloning them is safe.
- Record every rejection reason in an optional optimization remark so
  `-Winline` can report a useful source location without making ordinary builds
  noisy.
- Benchmark compile time, code size, and runtime before assigning distinct
  budgets to `-O2` and `-O3`.

CCC-IR must continue to own only transformations that require C type/effect
knowledge or canonical verified IR. Global value numbering, loop transforms,
machine instruction combining, register allocation, scheduling, and
target-specific peepholes remain Cranelift work; enabling inlining must not
create second implementations of those passes.

### Debug information and evidence

Inlining must not publish misleading source information. Until CCC emits
correct abstract origins, call-site ranges, and `DW_TAG_inlined_subroutine`
entries, keep heuristic inlining disabled for `-g` builds. Then add GDB and
LLDB checks that can step from caller to inlined callee, inspect surviving
arguments and locals, and produce a source-level backtrace.

The functional matrix must cover calls that are retained and removed,
attributes, recursion, address-taken functions, static and external linkage,
TLS, volatile and atomic effects, runtime-sized storage, computed goto, inline
assembly, variadics, ABI bridges, and returns-twice behavior. Exact CLIF
goldens, object-symbol checks, deterministic rebuilds, all target oracles,
Csmith, and the real-code corpora must agree at `-O0`, `-O2`, and `-Oz`.

## Remaining compiler work

### Language, value, and ABI coverage

- Implement binary128 `long double` constants, arithmetic, comparisons,
  conversions, fixed and variadic calls, aggregate boundaries, and `va_arg` on
  AArch64 and RISC-V Linux. Prove both cross-link directions with GCC and Clang
  and execute under native or QEMU environments before removing the current
  diagnostics.
- Implement `_Complex` and `_Imaginary` with typed pair operations,
  floating-point edge-case tests, compatible headers and builtins, and
  per-target fixed, variadic, return, and aggregate ABI plans. Remove
  `__STDC_NO_COMPLEX__` only for profiles that pass the complete contract.
- Complete the C atomic surface. Add verified native or `libatomic` provider
  contracts for the currently rejected representations, lower each accepted
  memory order accurately instead of strengthening everything to sequential
  consistency, and implement standard scaled pointer operations.
- Extend GNU `__int128` values and boundaries to the enabled AArch64 and RISC-V
  profiles, with the same helper, varargs, aggregate, and cross-link evidence
  required on x86-64.
- Implement old-style identifier-list function definitions that remain valid
  in the selected C language modes, and complete ISO and GNU inline-definition
  linkage independently from optimizer inlining.
- Promote commonly encountered GNU constructs from parse-only or unsupported
  states in measured corpus order. The first candidates are `typeof`,
  `gnu_inline`, function optimization attributes, missing integer builtins, and
  target-specific inline assembly. Each registry promotion needs typed
  semantics and object or execution evidence.

### Debugging

- Reconstruct nested lexical scopes rather than emitting one function-wide
  block.
- Track optimized SSA/register values with sound location lists and describe
  runtime-sized or dynamically realigned objects where the target DWARF model
  permits it.
- Add independently verified TLS location expressions for AArch64 and RISC-V
  ELF.
- Represent inlined calls, call sites, and abstract origins, and expand debugger
  tests from breakpoint/backtrace smoke checks to stepping and value
  inspection.

### Native stack and hardening

- Pursue an upstream-backed dynamic-stack operation that composes with fixed
  slots, spills, calls, stack probes, unwind information, and DWARF. Only then
  enable GNU `alloca` and an optional native-stack VLA provider.
- Define arena checkpoint behavior across returns-twice calls so a function can
  safely combine runtime-sized automatic storage with `setjmp`-family control
  flow.
- Implement stack protectors, stack-clash protection, and target control-flow
  hardening now accepted only as degradable flags. Verify generated prologues,
  failure paths, unwind behavior, and large-frame probing on every applicable
  target.

### Driver, assembly, and target breadth

- Add faithful target assembly output for `-S`; annotated disassembly must
  remain a separately named output. Round-trip assembly must preserve symbols,
  sections, relocations, visibility, CFI, and debug information.
- Broaden inline assembly through per-target register and constraint models.
  Add symbolic operands and `asm goto` only after CCC-IR can represent their
  control-flow and memory effects exactly.
- Add safe ordered assembler-option forwarding and the remaining dependency
  generation behavior only through explicit flag contracts, never a blanket
  unknown-option allowlist.
- Enable the catalogued x86-64 musl target with its own headers, CRT, TLS,
  linker, debug, and execution evidence. Keep Windows, 32-bit, big-endian,
  kernel, and freestanding profiles out until each has a complete target
  proposal rather than an architecture-only codegen switch.

### Validation breadth

- Run portable Lua, zlib, and zstd contracts on every applicable enabled
  target instead of reserving most real-code evidence for x86-64.
- Extend Csmith reference consensus and execution to AArch64 and RISC-V Linux.
  Keep native Darwin coverage and retain per-target tool identities and failure
  artifacts.
- Turn musl, tcc, and c-testsuite from catalogue entries into pinned,
  hash-verified adapters with explicit preprocess/compile/link/run contracts.
- Add persistent fuzz targets for preprocessing-token expansion, parser
  recovery, semantic analysis, the CCC-IR verifier, and optimizer
  idempotence. Generated valid programs do not cover malformed-input
  robustness.

## Smaller cleanups

- Split the largest implementation files along their existing domains:
  semantic declarations/types/expressions/statements, IR places/values/control
  flow, codegen values/calls/runtime effects, link planning/packaging, and
  driver planning/execution. Keep these moves behavior-neutral and require
  unchanged goldens and oracles.
- Extract target-neutral ABI helpers duplicated by the System V AMD64,
  AArch64, and RISC-V classifiers, while leaving each psABI classifier and its
  allocation rules independent. Snapshot ABI-plan digests before and after.
- Expose one enabled-target catalogue from `ccc-target` and generate or check
  the Rust test lists, corpus applicability table, scripts, and documentation
  from it. Adding a target must not require hand-editing several unrelated
  arrays.
- Add a shared Rust integration-test support module for RAII temporary
  directories, compiler invocations, retained failure artifacts, and command
  diagnostics. The driver tests currently repeat these utilities.
- Unify temporary artifact ownership across the driver and linker. Replace the
  private `_debug_workspace` lifetime side effect in `PackagingReport` with an
  explicit retained-debug-input guard whose cleanup is tested on success,
  failure, and signals.
- Create a typed diagnostic-code registry and generate uniqueness, ownership,
  range, and documentation checks while keeping serialized `CCCxxxx` strings
  stable.
- Generate the GNU capability tables and user-facing status summary from the
  compatibility registry, or fail CI when the prose and registry disagree.
- Factor the common audited compiler-wrapper mechanics used by the bzip2,
  Redis, SQLite, and zstd adapters into a shared tested tool with
  corpus-specific policy hooks.
- Split the large target-oracle and corpus shell runners into shared libraries
  plus target/corpus entry points. Derive executed-case totals from declared
  cases instead of maintaining numeric totals beside the tests.
- Add fast CI gates for formatting, Clippy with warnings denied, shell static
  analysis, Markdown/link validation, and whitespace checks before the
  expensive target and corpus jobs.
