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

## First: keep Cranelift `main` integration reproducible

CCC resolves `cranelift-codegen`, `cranelift-frontend`, `cranelift-module`,
`cranelift-object`, and their transitive Cranelift crates from the
[Wasmtime repository's `main` branch](https://github.com/bytecodealliance/wasmtime/tree/main/cranelift).
The committed `Cargo.lock` pins one exact revision, the ABI configuration key
records that revision as backend provenance, and a lockfile test rejects mixed
Cranelift sources or revisions. [ADR-0008](adr/0008-pin-cranelift.md) defines
the reproducibility and single-owner unwind policy.

The scheduled compatibility workflow refreshes an ephemeral candidate lockfile,
synchronizes candidate provenance, and tests upstream head without modifying
normal builds. Backend construction, settings, frontend finalization, empty
memory flags, symbol materialization, and custom data sections now pass through
one narrow compatibility module; CLIF instruction selection and optimization
remain direct. The remaining integration work is to:
- Extend the scheduled candidate beyond the workspace suite and all target
  oracles to dedicated debugger checks, ABI cross-links, Csmith profiles, and
  real-code corpus gates.
- Audit changes to signatures, legalization and verification order, atomics,
  object relocations, unwind information, debug value locations, and target
  flags before accepting each lockfile update. A newly available API is not an
  enabled CCC capability by itself.
- Keep `cargo tree` evidence with each accepted refresh and make the smallest
  failing target/corpus command available for upstream regression bisection.

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
- Finalize raw frontend-built CLIF and verify caller and candidates before
  invoking `Context::inline`; verify the rewritten caller again, then pass it
  through the normal module definition and Cranelift optimization pipeline.
  Do not invent a legalization or candidate pre-optimization pass: current
  Cranelift removed the public legalizer and its own inliner tests exercise this
  order. Require exactly the signature referenced by each call site. The
  inliner remaps global values, memory flags, and alias regions, so do not add a
  blanket rejection for those entities. Keep prepared bodies local to one
  target/configuration invocation; never reuse them across ISA or codegen
  settings.
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
- Enforce the recorded exact `always_inline` and `noinline` properties in the
  inlining policy, and diagnose an `always_inline` call that cannot be honored.
  Treat the separate C `inline` specifier as a hint, not as permission to
  ignore its C11 definition and linkage rules.
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

## Benchmarking and code-generation performance

Optimization work needs reproducible measurements at the CCC-IR, CLIF,
machine-code, and execution boundaries. The checked-in C-Ray adapter establishes
the first executable benchmark with pinned inputs, native reference agreement,
correctness checks, raw resource measurements, and machine-readable results.
Generalize that evidence model before changing inlining heuristics or making
broad code-generation changes: build a release compiler, record the
compiler/backend revision and complete target configuration, use deterministic
inputs, perform warmups and repeated samples, and compare results with a
previous revision. Keep correctness checks enabled in every executable
benchmark so faster wrong code can never appear as an improvement.

### Benchmark set

Keep the suite small enough for regular development while covering different
sources of compiler and generated-code cost:

- A minimal `int main(void) { return 0; }` translation establishes the fixed
  frontend, ABI, CLIF, object, and link overhead.
- Separate `puts("hello")` and `printf("hello\n")` programs expose direct
  external calls, string data, relocations, and the generated variadic-call
  protocol without mixing them together.
- A declaration-heavy translation includes a large hosted header surface but
  references only one function and one object. Its CLIF size must scale with
  used declarations, not every declaration visible in the translation unit.
- Focused, defined-behavior kernels cover direct calls, inlining, integer and
  floating loops, branches and switches, loads and stores, aggregate copies,
  TLS, atomics, and variadic calls. Each kernel validates its result and has a
  fixed work count.
- Generated scaling cases vary function count, declarations per function,
  block count, SSA values, globals, and string literals independently. Use
  them to detect accidental quadratic behavior and peak-memory growth.
- Whole-program measurements use the existing bzip2, zlib, and zstd adapters
  with fixed inputs. Record translation time, link time, aggregate object
  size, executable text size, and execution throughput without weakening their
  correctness contracts.
- Extend the C-Ray result schema with CCC frontend and Cranelift phase timing,
  CLIF instruction/block/stack-slot counts, and parsed object-section sizes.
  The existing correctness and performance profiles already retain whole
  compile/link/render timing, CPU, peak-memory, file-size, command, image-hash,
  and exact same-host reference evidence.

Run IR and object-size measurements for every enabled target. Runtime
comparisons are native-target evidence; QEMU runs remain correctness and rough
trend evidence and must not be compared numerically with native execution.

### Metrics and instrumentation

- Report preprocessing, parsing, semantic analysis, CCC-IR lowering and
  optimization, ABI planning, CLIF lowering, Cranelift compilation, object
  packaging, and linking separately. Also record end-to-end wall time, CPU
  time, and peak resident memory.
- Count CCC-IR functions, blocks, values, operations, and dead operations
  before and after CCC-owned optimization. Count CLIF blocks, instructions,
  stack slots, signatures, external function references, global values, and
  how many imported entities are never used.
- Record emitted text, read-only data, writable data, debug-section, unwind,
  relocation, and symbol-table sizes. Keep debug and non-debug measurements
  separate.
- Record runtime distributions rather than one timing. Use a pinned native
  runner for regression decisions, retain raw samples, and reject comparisons
  whose noise or confidence interval is larger than the claimed change.
- Compare generated code with the previous CCC revision at all optimization
  levels. Where GCC and Clang support the same source contract, also record
  their `-O0`, `-O2`, and size-optimized results as directional references,
  not as substitutes for CCC correctness.

### Initial performance targets

- Including hosted headers in the two hello programs must not change their
  per-function CLIF instruction or imported-entity counts compared with the
  equivalent minimal declarations.
- Variadic-call setup must initialize only protocol fields and argument bytes
  that a helper can read. It must not clear the complete maximum-size frame
  byte by byte. Reduce the checked-in `printf` CLIF instruction baseline by at
  least 90%, and require setup instruction and store counts to scale with live
  arguments rather than frame capacity.
- On the defined-behavior kernel suite, the `-O2` runtime geometric mean must
  not regress against `-O0`, and `-Oz` text size must not exceed `-O2` in the
  geometric mean. Record and justify individual exceptions rather than hiding
  them in the aggregate.
- After the first stable baseline, fail the dedicated benchmark job for a
  greater than 5% regression in compiler-time or runtime geometric mean, a
  greater than 10% increase in peak memory, or a greater than 5% increase in
  text size. Require repeated confirmation before updating a baseline.
- As an initial competitive code-quality goal, keep CCC `-O2` within 1.5 times
  the faster of GCC and Clang for runtime and within 1.25 times the smaller
  reference text size on the scalar integer, branch, call, and memory kernels.
  Track vectorization-dependent cases separately until CCC has an explicit
  vectorization strategy.
- Apply the suite-wide 5% regression limit to C-Ray compile time, text size,
  and render time after its first stable baseline. Bring CCC `-O2` within 1.5
  times the faster same-host strict-FP GCC/Clang render time and within 1.25
  times the smaller reference text size before tightening the runtime target.

Publish benchmark summaries for pull requests without making noisy shared
runners authoritative. A scheduled run on pinned hardware owns regression
decisions and retains historical results. Use profiles and flamegraphs to
optimize the hottest CCC-owned stages; do not recreate transformations already
performed by Cranelift merely to improve a benchmark score.

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
