# Testing and correctness strategy

Correctness is checked at each explicit compiler boundary and at binary interfaces with independent compilers.

## Test tiers

- **Unit/property tests:** token formation, macro expansion, declarators, type/layout rules, constant evaluation, IR invariants, target configuration, and ABI classifiers.
- **Snapshot tests:** deterministic preprocessed output, tokens, AST, typed AST, CCC-IR, ABI plans, CLIF, diagnostics, effective configuration, and link plans.
- **Execution tests:** compile, link, run, and assert exit status/stdout/stderr for every executable target environment.
- **Compile-fail tests:** stable diagnostic codes, primary spans, macro/include provenance, and essential wording; incidental formatting is normalized.
- **Object/disassembly tests:** sections, symbols, visibility, relocations, TLS, DWARF, calling-sequence details, PIE/PIC behavior, and generated bridges.

Preprocessor fixtures separately cover normalization, object-like and
function-like macros, conditionals, computed includes, include ordering,
pragmas, predefined macros, dependency output, linemarkers, and provenance.
Focused regressions cover UTF-8 literal scanning, prefixed character values,
catch-all preprocessing tokens, directive comments spanning physical lines,
direct and computed header predicates, terminating recursive inclusion, and
identity-aware depth diagnostics.
Inactive conditional groups are checked against a reference preprocessor for
both valid skipped code and malformed preprocessing tokens. `#warning` tests
pin warning promotion and message normalization. Linemarker tests cover entry,
return, system-header state, `#line`, `-P`, and recompilation of emitted `.i`
text.

## ABI oracle

For every target:

- generated classifier tests compare CCC layout and call classification with the published psABI and independently inspect GCC/Clang output;
- cross-linked tests compile CCC callers with reference callees and reference callers with CCC callees;
- aggregate, hidden return, register exhaustion, stack alignment, variadic, TLS, long-double, and helper-call cases are included;
- matching CCC on both sides is never treated as proof of platform ABI compatibility.

The enabled x86-64 Linux layout gate requires both GCC and Clang, records each
compiler identity, and rejects a reported target that is not ABI-compatible
with `x86_64-unknown-linux-gnu`. It compares ELF symbol sizes plus XOR deltas
from zero-initialized baselines, so padding bytes are not treated as values.
The bit-field corpus includes mixed declared types, signed negative values,
zero-width barriers, storage-unit boundaries, unnamed fields, nested records,
unions, and packed records. A missing reference compiler is a test failure,
not a skipped oracle.

The `x86_64-unknown-linux-gnu` classifier generator records seed
`0x4343435f41424931`. It materializes 45 type recipes from 11 structural
families: 32 integer-byte aggregate sizes, one float record, four double-array
sizes, and eight fixed record, union, nested, bit-field, and packing recipes.
Those recipes are crossed with deterministic leading/trailing GP and SSE
allocator-pressure patterns to produce 4,096 distinct canonical plan inputs;
this count does not claim 4,096 unrelated aggregate layouts. Named boundary
regressions are always selected. Remaining inputs are partitioned by passing
mode, return mode, size bucket, packing, mixed-class status, and
register-exhaustion shape. The 256-case cross-link set is chosen by sorting
canonical encodings by a domain-separated SHA-256 digest within each bucket
and visiting bucket keys in lexicographic round-robin order. The selector test
proves every declared nonempty bucket is represented and snapshots the
selected identifiers.

Variadic tests separately cover call sites and definitions, direct and indirect
calls, explicit `float`-to-`double` and character/short-to-`int` default
promotions through ellipses, aggregates, register/stack
boundaries, `va_copy`, and target-specific state such as SysV `%al`. CCC-created
lists are consumed by libc `vsnprintf` and `vfprintf`; GCC- and Clang-created
lists are consumed and copied by CCC. Exact formatted-output checks run under
the `C` locale. Generated CFI is exercised by the real libgcc
`_Unwind_Backtrace` API across both generated bridge kinds in addition to the
debugger checks. Long-double tests verify
size/alignment/macros and object representation; profiles that support boundary
transport also test calls in both directions, while profiles that do not must
reject those boundaries exactly. Explicit compatibility-mode objects must be
rejected when mixed with incompatible CCC objects.

Planner, IR, digest, renderer, fake-command, and manifest tests run on every
host. Native `x86_64-unknown-linux-gnu` execution and object suites compile only
on Linux x86-64. The required CI feature rejects the wrong host at compile time,
preflights every reference and packaging tool, and invokes each named native
test binary explicitly. A missing tool or zero-test configuration is a hard
failure, never an implicit skip.

## Runtime-sized automatic storage

Provider-independent tests first prove that nonconstant array extents are
evaluated once and retained in the typed AST, including parameter-order binding,
and multidimensional pointer strides. Diagnostics distinguish prototype-scope
`[*]`, legal fixed-size objects with variably modified types, nonautomatic VLA
objects, invalid bounds, and illegal storage classes. Runtime `sizeof` and
control-flow ingress that bypasses a declaration remain explicit gates before
the complete VLA capability can be advertised.

CCC-IR tests pin runtime storage identities, allocation effects, retained
extents, dynamic pointer strides, verifier type/dominance checks, and append-only
digest tags. Execution fixtures cover bound-once evaluation, multidimensional
access, normal return cleanup, recursion, concurrent invocations, and
over-alignment. Named/switch/computed-goto ingress and runtime `sizeof` remain
negative capability gates rather than silently approximated behavior.

Provider tests inject nonpositive bounds, extent and alignment overflow,
allocation failure, and alignments through over-aligned `_Alignas` declarations.
On System V AMD64 every VLA allocation is checked for the target's 16-byte
minimum as well as stronger declared alignment. An instrumented descending-size
loop proves that one growth serves every later execution of the declaration and
that normal return releases it once. An external GCC default-PIE link runs the
same provider surface under AddressSanitizer and LeakSanitizer. Native x86 tests
execute the nonpositive and overflow traps; object inspection verifies their
`ud2` failure paths when the development host's amd64 emulator cannot deliver
those trap signals correctly.

Returns-twice and cross-language unwinding remain separate hard gates. A
nonlocal exit may strand the abandoned invocation's cached allocations, as C
permits for VLA storage, but cannot share or corrupt another invocation's state.
The provider is not async-signal-safe because growth calls the hosted allocator.

Object and disassembly checks prove that affected user functions keep their
ordinary Cranelift frame and import only the declared hosted dependencies. A
mixed-link test links a CCC-produced object with an external GCC-compatible
driver as PIE. Negative feature-predicate tests keep `__builtin_alloca`
unavailable for an arena-only profile.

## C11 and GNU capability fixtures

Statement-expression tests distinguish GNU C's three result categories rather
than treating every closing expression alike. Transparent
top-level-unqualified scalar and aggregate places and eligible bit-field places
are tested as lvalues. Top-level `const` and `volatile` ordinary finals are
tested as non-lvalues, including exactly one ordered read when materializing a
volatile final. Aggregate forwarding is tested for preservation of nested
member and pointed-to qualifications. Bit-fields declared with access
qualifiers retain their descriptor and remain non-addressable; modification of
a forwarded `const` bit-field is diagnosed, while a forwarded `volatile`
bit-field remains assignable. Separate fixtures select an unqualified bit-field
through `const`- and `volatile`-qualified aggregate places and require
non-lvalue results. Arrays and functions are tested for decay;
declaration-bearing and multi-statement bodies are tested for value capture;
and bodies without a retained final expression are tested as `void`. GCC's
inconsistent same-type-cast generalized-lvalue behavior is rejected unless it
is adopted by an explicit compatibility decision. `_Generic` has a separate
matrix proving controlling-expression conversions and preservation of the
selected association's value category.

Computed-goto fixtures include direct, copied, automatic-table, and
static-table `&&label` dispatch. Exact IR goldens prove that mutable locals in a
function containing a computed jump remain in storage and that every
`br_table` destination has no arguments or block parameters. Negative fixtures
diagnose label subtraction and base-label-plus-offset reconstruction while the
direct operands still retain label provenance. Arithmetic after a token has
passed through object storage is outside the supported behavior and is not
claimed as a diagnostic boundary.

Wide-integer proof covers high-bit constants, signed and unsigned arithmetic,
division and remainder traps, floating conversions, layout, varargs, mixed
register pressure, and GCC/Clang cross-linking in both directions. Object and
link-plan checks require the exact `ti` helper symbols selected by each
operation. No corpus result substitutes for this matrix.

## Differential testing and undefined behavior

Differential tests compare only outputs whose relevant behavior is defined for the identical effective configuration.

- Csmith supplies generated defined-behavior programs; generator version, options, seed, and target assumptions are recorded.
- Handwritten differential tests include a short statement of the standard/extension rule that makes the observed result defined. Tests with unspecified evaluation order, padding bytes, pointer ordering outside one object, uninitialized data, data races, or implementation-defined settings not fixed by the configuration are excluded from output comparison.
- GCC and Clang are run at multiple optimization levels. UBSan and ASan runs are useful bug detectors but are not accepted as proof that a program is free of undefined or unspecified behavior.
- Metamorphic tests compare semantics-preserving variants without relying on a single reference compiler.
- C-Reduce reductions preserve an interestingness predicate that reruns all definedness checks; reduced fixtures retain provenance and license metadata.

## Target execution matrix

- x86-64 Linux GNU and musl run natively in matching environments;
- AArch64 Linux and RISC-V64 Linux run natively where available and under matching QEMU user/system environments for cross coverage;
- Darwin arm64 runs on native macOS CI with the selected SDK/deployment target;
- compile-only object inspection supplements execution but never substitutes for a claimed runnable target.

Every enabled target has a required matrix entry. A target without an execution environment is labeled compile-only and cannot be advertised as execution-tested.

## Real-code corpus

SQLite, Lua, Redis, bzip2, zstd, zlib, musl, tcc, selected libc-header
fixtures, and c-testsuite exercise drop-in compatibility. Each integration
records the exact build command, enabled features, patches if any, expected
exclusions, and whether success means preprocess, compile, link, or run.
“Builds unmodified” is used
only when no source or build-system patch is applied.

Hosted-header preprocessing, parsing, and code generation are separate gates.
A licensed, pinned glibc-like fixture has both a deterministic preprocessing
golden and an AST surface check. On x86-64 Linux, installed glibc feature,
definition, integer, type, unistd, string, and pthread headers are exercised by
preprocessing and parsing tests plus a compile-link-execute sentinel. The
installed-header tests record compiler, target, and libc identity and assert
stable sentinel properties rather than snapshotting a mutable system header.
Parse-only declarations remain confined to the AST fixture; the execution
sentinel independently proves the supported declaration subset.

Corpus pins encode capability dependencies. SQLite is pinned by its
[corpus manifest](../../test-corpus/sqlite/manifest.toml) to 3.47.2. Release
3.47.0 removed SQLite's remaining `long double` use by switching that
calculation to Dekker's algorithm; an earlier release silently requires the f80
runtime on the primary target. This pin is also the last canonical-source
release before SQLite replaced the classic Autoconf configure/Makefile
interface with Autosetup in 3.48.0; any later pin requires an adapter-interface
review as well as a C-surface inventory. The default gate runs the `veryquick`
Tcl set through `testfixture`, which needs Tcl and zlib development
environments. Explicit `quick`, `all`, and `full` adapter modes retain the
upstream test grouping; TH3 remains out of scope. Under CCC's effective identity
the build selects `__sync_synchronize` but no inline assembly, wide integers,
VLA objects, computed goto, or statement expressions. The `full` mode keeps
SQLite's `SQLITE_ENABLE_STMT_SCANSTATUS` fuzzcheck profile. For its one
generated `sqlite3.c` translation, the wrapper defines the upstream
`__STRICT_ANSI__` predicate while retaining GNU C11 mode, selecting SQLite's
zero-valued hardware timing fallback instead of the GNU x86-64 `rdtsc`
inline-assembly path. In this release that predicate also suppresses only the
`SQLITE_INLINE` optimization hint. The eight fuzzcheck support inputs,
`alltest`, and `sessionfuzz` receive no override. The wrapper audits the last
effective `-std` option and predicate state for every translation. Corpus
success is integration evidence rather than proof of the unselected constructs;
their focused fixtures remain required.

The `all` and `full` Make targets also build SQLite's command-line shell. Its
`NAN` use expands through the hosted glibc `<math.h>` to
`__builtin_nanf("")`; the same header surface exposes `INFINITY` through
`__builtin_inff()`. Focused typed-AST, exact CCC-IR, and execution fixtures
prove that these calls are constant expressions with canonical binary32 quiet
NaN and positive-infinity bits. The NaN contract accepts only an empty ordinary
or UTF-8 string literal. Nonliteral, wide, and nonempty payloads are rejected,
so passing the corpus cannot hide unimplemented GNU payload encoding.

Lua is pinned by its [corpus manifest](../../test-corpus/lua/manifest.toml) to
the official 5.5.0 source and matching test archives. The adapter uses the
upstream Linux make target in GNU11 mode and requires all 34 `.c` files in the
source directory to appear exactly once in CCC's source-input log. GCC receives
only CCC-produced objects and archives for the two final program links. Because
CCC's selected relocation model is static, both links use Lua's
`MYLDFLAGS=-no-pie` hook and the gate verifies ELF `EXEC` type plus the absence
of dynamic text relocations.

Under the pinned GNU 4.2.1 identity, Lua selects `__builtin_expect`, internal
visibility and noreturn attributes, `__extension__`, and computed-goto VM
dispatch; hosted `float.h` and `math.h` additionally select binary64 target facts
and `__builtin_huge_val`. The Linux profile uses `_setjmp`/`_longjmp` for
protected calls. The execution gate invokes the official basic profile exactly
as `lua -e'_U=true' all.lua`, after removing ambient Lua initialization and
module-path variables, and requires its `final OK !!!` marker. This integration
does not replace focused computed-goto or nonlocal-control tests. The official
complete and internal profiles remain distinct contracts because they add
position-independent shared test modules, GNU assertion statement expressions,
and an instrumented runtime. musl feeds `$CC` assembly files.

Redis is pinned by its [corpus manifest](../../test-corpus/redis/manifest.toml)
to the official 8.8.0 core archive. The selected upstream build produces
`redis-server` and `redis-cli` with the libc allocator while disabling TLS,
systemd, optional vector sets, bundled data-type modules, and link-time
optimization. All 178 selected C translation units pass through CCC in GNU11
mode; native GCC receives only the resulting objects and archives for two
non-PIE links. The wrapper mirrors each real translation with a preprocessing
pass under the same effective language, macro, include, warning, and
optimization arguments. The adapter requires exactly 178 nonempty captures,
compares their relative paths to the pinned source set, and records exact
expanded-builtin counts. It also audits the complete compiled source multiset,
compiler identity, link inputs, ELF executable type, and absence of dynamic
text relocations.

Redis assertions remain enabled. A forced compatibility `assert.h` selects
glibc's standard-C macro and uses the C11 `__func__` identifier for the
diagnostic function name, then restores GNU mode for the source build. Six
exact-hash source adjustments express Redis's expression-valued sync-CAS loop
through a standard-C helper, replace the bundled HDR Histogram x86 atomic
assembly with the selected sequentially consistent legacy builtins, and express
binary64 classification in hiredis and the bundled Lua extensions without
semantically visiting glibc's native-`long double` generic arms. The sixth
selects a behavior-compatible standard-C no-op for xxHash's compiler guard
under CCC instead of the GNU inline-assembly guard chosen by the compatibility
tuple. The adapter
audits each removed construct and its replacement count explicitly.

The Redis execution profile starts the CCC-built server on a private Unix
domain socket and drives `PING`, string, counter, list, hash, Lua `EVAL`, and
database-size checks through the CCC-built client. It is deliberately a
focused build-and-smoke contract: the upstream full test suite is not invoked.

bzip2 is pinned by its [corpus manifest](../../test-corpus/bzip2/manifest.toml)
to the official 1.0.8 archive and a full commit and tree from the official
`bzip2-tests` repository. The upstream Makefile selects nine of the archive's
13 C files to build `libbz2.a`, `bzip2`, and `bzip2recover`; CCC must compile
each selected input exactly once, and native GCC performs only the two
source-free non-PIE program links. The build inventory also proves that the
four developer utilities were not silently substituted into the product set.

The bzip2 execution gate combines the six byte-for-byte comparisons from
upstream `make check`, an independent level-9 integrity/round-trip fixture,
and the pinned extended runner over 38 valid and eight deliberately malformed
streams. The extended profile requires exactly 440 pass records and its final
success marker. Optional Valgrind discovery is disabled explicitly; decoder,
corruption, small-memory, and recovery behavior remain exercised.

zstd is pinned by its [corpus manifest](../../test-corpus/zstd/manifest.toml) to
the official 1.5.7 release archive. Its selected `make check` build routes 50
C translation occurrences through CCC, including two exact-byte pthread
capability probes produced during recursive Make evaluation. Native GCC sees
only objects for four source-free links. Pthread support and legacy decoding
are enabled; optional zlib, liblzma, and liblz4 format wrappers, stand-alone
assembly, and host-dependent unaligned scalar accesses are disabled.

The zstd adapter applies an exact-hash extension of upstream's no-assembly
guards to existing generic C fallbacks, supplies the documented
`ZSTD_DEPS_COMMON` libc-memory boundary, and forces a portable assertion header
without changing GNU identity. Every adjustment and compatibility header is
hashed and audited on each run. Upstream's bounded quick smoke target covers
compression, decompression, streaming, dictionaries, file handling, corruption
rejection, sparse files, and the selected threaded path; deterministic file and
stream round trips add byte-for-byte checks. Long-running fuzz and stress
profiles are not part of this gate.

A curated execute-only compiler torture subset may supplement focused fixtures.
It is fetched rather than vendored, and its corpus manifest records the exact
upstream revision, selected paths, license, hashes, required language profile,
and exclusions for undefined or implementation-specific behavior. Passing that
subset does not replace exact typed-AST, IR, ABI, and execution regressions for
the supported constructs.

## Licensing, pinning, and supply-chain policy

- Csmith uses its BSD-style license; C-Reduce uses the University of Illinois/NCSA-style license; GCC tests are governed by the applicable GCC GPL terms. Each corpus/tool is recorded individually instead of being grouped under one license label.
- Only compatible, small fixtures are vendored. External corpora are fetched by immutable revision and cryptographic hash into a CI cache or project-controlled mirror.
- Fetch scripts verify hashes and licenses before use; a network outage cannot silently select a different revision.
- Minimized fixtures derived from third-party tests retain the source license and provenance when the license requires it.
- External GPL tools may be executed without being linked into CCC; source-copying and distribution decisions are handled separately from tool execution.

Fast unit/snapshot/execution tiers run per change. Fuzzing, differential matrices, full corpora, QEMU, native Darwin, and exhaustive ABI shapes run on scheduled and release CI, with failures retained as reproducible seeds/artifacts.
