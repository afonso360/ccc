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
normal builds. It runs the workspace suite, every target oracle, bounded
structural-scaling axes, and all nine kernel objects at every optimization
profile and enabled target. Backend construction, settings, frontend
finalization, empty memory flags, symbol materialization, and custom data
sections now pass through one narrow compatibility module; CLIF instruction
selection and optimization remain direct. The remaining integration work is
to:
- Extend the scheduled candidate to dedicated debugger checks, ABI
  cross-links, Csmith profiles, and real-code corpus gates.
- Audit changes to signatures, legalization and verification order, atomics,
  object relocations, unwind information, debug value locations, and target
  flags before accepting each lockfile update. A newly available API is not an
  enabled CCC capability by itself.
- The scheduled artifact now retains the exact candidate revision and complete
  workspace `cargo tree` beside its lockfile/provenance patch, target oracles,
  and benchmark evidence. Make the smallest failing target or corpus command
  available for upstream regression bisection next.

## Second: broaden and measure Cranelift inlining

CCC now uses Cranelift's
[`Context::inline`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/context.rs)
and
[`Inline` policy interface](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/inline.rs).
Cranelift provides the transformation but deliberately leaves call-graph
knowledge and heuristics to its user, so CCC must supply the policy and the
proof that each selected call is safe. The first slice prepares and verifies
all raw CLIF definitions before compilation, resolves exact namespace-zero
`FuncRef` signatures within one target invocation, and admits bounded strong
internal native leaf definitions at `-O2`/`-O3`. Exact `noinline` is enforced;
safe `always_inline` leaves are required at every optimization tier and unsafe
uses receive `CCC4012`. Rewritten callers still enter the ordinary Cranelift
definition/optimization pipeline exactly once, and original out-of-line
definitions remain emitted.

### Remaining code-generation policy

- Extend the candidate proof beyond leaves only after focused evidence for
  non-leaf bodies, hidden/final external definitions, global values, stack
  slots, alias regions, TLS, volatile and atomic effects, computed goto,
  runtime-sized storage, and supported inline assembly. Imports, weak or
  interposable definitions, generated ABI bridges, incompatible signatures,
  indirect calls, and patchable calls must remain out unless their distinct
  semantics acquire an explicit contract.
- Keep user-named symbolic global values out until Cranelift remaps their
  function-local user-name references while cloning global values. Track that
  upstream rather than maintaining a second CCC-side entity remapper.
- Replace the initial translation-unit ceiling—eight fully budgeted callers,
  bounded by 64 optional sites, 768 estimated instructions, and 128 estimated
  blocks—with benchmark-derived, separately tuned `-O2` and `-O3` limits. The
  deterministic depth-one policy now has exact per-callee, per-caller, and
  whole-translation-unit bounds but deliberately has no profile-specific
  tuning.
- Decide whether `-Os`/`-Oz` may inline only after measuring a net encoded-size
  reduction rather than assuming raw CLIF counts predict machine-code size.
- Broaden the set of safely honor-able `always_inline` definitions. Continue
  diagnosing every referenced required body outside that set instead of
  silently weakening the attribute.

### Policy and language semantics

- Record every rejection reason in an optional optimization remark so
  `-Winline` can report a useful source location without making ordinary builds
  noisy.
- Complete ISO and GNU inline-definition/linkage semantics independently of
  optimizer choice. The C `inline` specifier is currently only a size-budget
  hint and must never become permission to change an externally observable
  definition.
- Keep returns-twice callers and callees, recursive SCCs, generated bridge
  bodies, and other exceptional frame contracts out until focused ABI,
  unwind, and execution tests prove that cloning them is safe.

CCC-IR must continue to own only transformations that require C type/effect
knowledge or canonical verified IR. Global value numbering, loop transforms,
machine instruction combining, register allocation, scheduling, and
target-specific peepholes remain Cranelift work; broadening inlining must not
create second implementations of those passes.

### Debug information and evidence

Heuristic inlining is disabled for `-g`, and a required `always_inline` call is
diagnosed, so the first slice cannot publish misleading source information.
Emit correct abstract origins, call-site ranges, and
`DW_TAG_inlined_subroutine` entries before lifting that restriction. Then add
GDB and LLDB checks that can step from caller to inlined callee, inspect
surviving arguments and locals, and produce a source-level backtrace.

The initial all-target matrix covers calls that are retained and removed,
attribute requirements, recursion, address-preserving out-of-line symbols,
static and external linkage, weak definitions, indirect calls, generated
bridges, returns-twice callers, debug builds, deterministic rebuilds, and exact
budget boundaries. Extend it to TLS, volatile and atomic effects,
runtime-sized storage, computed goto, inline assembly, deeper variadic cases,
exact CLIF goldens, target-oracle execution, Csmith, and the real-code corpora
at `-O0`, `-O2`, and `-Oz`.

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
sources of compiler and generated-code cost. The checked-in compiler-only
runner now measures minimal return, separate `puts` and variadic `printf`
calls, a paired minimal/hosted `<stdio.h>` translation, independent
zero-to-1,024 unused function and data declaration axes, and 8-to-128 live
functions at each optimization level. The hosted translation references only
`fputs` and `stdout`; every post-inlining CLIF and primary-object metric must
match the equivalent minimal declarations at the same target and optimization
profile. Preprocessing and semantic-analysis work may grow, but code-generation
structure may not. The runner retains raw timing/RSS samples and the complete
versioned codegen-stat stream. One shared CCC-IR requirement analysis now
materializes ObjectModule function, ordinary-data, and TLS declarations only
for definitions and retained direct, address, or static-initializer references;
unused TLS declarations also create no accessor artifact. The runner fails if
unused declarations change either post-inlining CLIF or primary-object
structural metrics. Timed samples use ordinary object-only compilation; the
structural-stat query is separate and untimed.

The current executable slice and next benchmark targets are:

- The compact nine-case defined-behavior suite is checked in: fixed-work
  direct-call/inlining, unsigned-integer, exact binary32/binary64,
  branch/switch, indexed load/store, 32-byte aggregate-copy, TLS, C11 atomic,
  and variadic-call workloads all self-validate. They run through separate
  object-only, correctness, and native-performance modes, and their versioned
  evidence keeps compiler-side primary-object statistics distinct from final
  packaged objects. Establish controlled native baselines and extend
  cross-target correctness execution next.
- Independent block-count, live-SSA-value, referenced-global, and
  string-literal scaling now joins the translation-unit and per-function
  declaration and live-function axes. The per-function family grows unused
  block-scope prototypes across a fixed live call graph while requiring every
  post-inlining CLIF and primary-object metric to stay identical. Each
  structural family rejects dead fixtures and superlinear growth. Correlate
  every axis with phase timing and peak-memory growth next.
- Whole-program measurements use the existing bzip2, zlib, and zstd adapters
  with fixed inputs. Record translation time, link time, aggregate object
  size, executable text size, and execution throughput without weakening their
  correctness contracts.
- Add CCC frontend and Cranelift phase timing to C-Ray. Its result schema now
  includes post-inlining CLIF instruction, block, live-value, call, stack-slot,
  signature, external-reference, and global-value counts plus parsed
  final-object section sizes. The correctness and performance profiles also
  retain whole compile/link/render timing, CPU, peak-memory, file-size, command,
  image-hash, and exact same-host reference evidence.

Run IR and object-size measurements for every enabled target. Runtime
comparisons are native-target evidence; QEMU runs remain correctness and rough
trend evidence and must not be compared numerically with native execution.

### Metrics and instrumentation

- `--emit=codegen-stats` now emits a versioned deterministic TSV view of
  post-inlining CLIF structure and the primary relocatable object on every
  enabled target. Schema version 2 includes the exact live CLIF value count:
  final-layout block parameters plus instruction results, excluding detached
  data-flow-graph entities. C-Ray archives that complete view and uses the CLIF
  subset in its summary; its independent final-object parser accounts for
  generated bridge packaging.
- Report preprocessing, parsing, semantic analysis, CCC-IR lowering and
  optimization, ABI planning, CLIF lowering, Cranelift compilation, object
  packaging, and linking separately. Also record end-to-end wall time, CPU
  time, and peak resident memory.
- Count CCC-IR functions, blocks, values, operations, and dead operations
  before and after CCC-owned optimization. CLIF live blocks, values,
  instructions, stack slots, signatures, external function references, and
  global values are implemented. Schema version 3 also records the allocated
  signature, external-function, and global-value entries left unreachable from
  live post-inlining CLIF and Cranelift's function-level semantic roots. Keep
  these as observational metrics rather than adding a duplicate cleanup pass.
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

- Keep the checked-in `fputs`/`stdout` and variadic `printf` hosted-header
  translations exactly equal to their minimal-declaration baselines at the
  post-inlining CLIF and primary-object boundaries.
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
- Extend the verifier-backed promoted-scalar location lists to optimized
  constants and composite values, and describe runtime-sized or dynamically
  realigned objects where the target DWARF model permits it.
- Add independently verified TLS location expressions for AArch64 and RISC-V
  ELF.
- Represent inlined calls, call sites, and abstract origins, and expand
  debugger tests beyond the current breakpoint, backtrace, and promoted-local
  value checks to stepping and scope-sensitive inspection.

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
- Extend the shared enabled-target catalogue beyond the Rust backend/header
  tests and the checked corpus-applicability catalog to scripts and
  documentation. Adding a target must not require hand-editing several
  unrelated arrays.
- Driver integration tests which allocate temporary trees now use the shared
  support module. Collision-safe RAII workspaces, retain-on-panic artifacts,
  command status diagnostics, target-driver discovery, and glibc identity are
  shared; diagnostics, link-input, visibility, object-emission, and execution
  tests use the common workspace. Preprocessing, hosted-header parsing, and the
  compact ABI oracle now use it as well, and the System V AMD64 interop suite
  completes the temporary-workspace migration. A minimal shared CCC command
  constructor now covers every non-System-V driver integration invocation
  while keeping targets, environments, working directories, and arguments
  visible at each call site. The System V AMD64 suite retains its own
  result-checking runner because that wrapper is part of its ABI-oracle
  contract.
- Mach-O debug-map inputs now have explicit ownership:
  `PackagingReport` exposes a must-use `RetainedDebugInputs` guard which the
  driver holds through final linking and `dsymutil`. Success, publication
  failure, and process-isolated signal cleanup are tested. A cross-host
  object-only Mach-O oracle now packages a debug primary with a generated TLS
  bridge, inspects raw OSO ownership and localized unwind-bearing helper state,
  and proves guard-controlled cleanup. Keep the native Darwin assembler,
  linker, `nmedit`, `dsymutil`, UUID, and LLDB oracle authoritative before
  extending OSO-bearing artifact lifetimes beyond one driver invocation.
- Create a typed diagnostic-code registry and generate uniqueness, ownership,
  range, and documentation checks while keeping serialized `CCCxxxx` strings
  stable.
- Generate the GNU capability tables and user-facing status summary from the
  compatibility registry, or fail CI when the prose and registry disagree.
- Factor the common audited compiler-wrapper mechanics used by the bzip2,
  Redis, SQLite, and zstd adapters into a shared tested tool with
  corpus-specific policy hooks.
- The target oracle now validates every completed case against a declarative,
  per-target case plan, and the fast quality job proves that skipped, reordered,
  missing, and extra cases fail with the exact plan position. Continue splitting
  the large target-oracle and corpus shell runners into shared libraries plus
  target/corpus entry points.
