# Testing and correctness strategy

Correctness is checked at each explicit compiler boundary and at binary interfaces with independent compilers.

## Test tiers

- **Unit/property tests:** token formation, macro expansion, declarators, type/layout rules, constant evaluation, IR invariants, target configuration, and ABI classifiers.
- **Snapshot tests:** deterministic preprocessed output, tokens, AST, typed AST, CCC-IR, ABI plans, CLIF, diagnostics, effective configuration, and link plans.
- **Execution tests:** compile, link, run, and assert exit status/stdout/stderr for every executable target environment.
- **Compile-fail tests:** stable diagnostic codes, primary spans, macro/include provenance, and essential wording; incidental formatting is normalized.
- **Object/disassembly tests:** sections, symbols, visibility, relocations, TLS, DWARF, calling-sequence details, PIE/PIC behavior, and generated bridges.

Debug-object inspection distinguishes `-g0` from every enabled debug level,
requires `.debug_info`, `.debug_abbrev`, `.debug_line`, range data, and the
existing call-frame section, and parses the emitted DWARF independently. It
checks compilation-unit, subprogram, core-type, member, parameter, variable,
and single function-wide lexical-block DIEs. Fixed frame locations, ordinary
data addresses, promoted-scalar location lists with assignment-boundary gaps,
x86-64 ELF and Darwin TLV TLS address expressions, prototype markers, multiple
source rows, and object-format relocations from debug sections to code, data,
TLS data, and other debug sections are inspected directly. Cross-target `-O2`
tests require every promoted source DIE, ordered non-overlapping ranges for
values the backend preserves, and conservative omission for unavailable
versions. Nested lexical ranges are not claimed.
The native Darwin gate also performs a one-step `-g` compile and link through a
generated TLS accessor, requires
the executable and staged `.dSYM` UUIDs to match, then uses LLDB to stop on a C
source line, recover both a fixed-frame local and the promoted scalar
`observed = 42`, and confirm the TLS definition's certified location
expression. A command/cleanup fixture
separately proves that `dsymutil` runs before registered link objects are
released and that incomplete bundle trees never replace an existing bundle.

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
reject those boundaries exactly. The native x86-64 matrix cross-links f80
fixed and variadic calls, `%st(0)` returns, memory-class aggregates, direct and
function-pointer calls, arithmetic, ordered and unordered comparison, integer
and binary32/binary64 conversions, volatile access, and `va_arg` with GCC at
both `-O0` and `-O2`.
Volatile half-ULP additions under `FE_UPWARD` and `FE_DOWNWARD` prove that the
localized x87 arithmetic observes and preserves the active control word.
An additional native gate uses the pinned
[Berkeley oracle manifest](../../tests/target-oracle/berkeley-testfloat.toml).
Official SoftFloat and TestFloat 3e archives are downloaded by exact byte
length, SHA-256, and SHA3-256, and their BSD-3-Clause license files are checked
before use. TestFloat's verifier and SoftFloat reference are built with GCC;
the small subject-operation object is produced by CCC and linked directly with
the pinned verifier objects.
Neither package is a compiler dependency or production-linked runtime.
The subject transfers TestFloat representation structs through
`__builtin_memcpy`, guarded by compile-time size equality, so no effective-type
or alignment assumption enters the comparison. For float-to-integer inputs
whose C cast would be undefined, the adapter first returns the pinned
8086-SSE specialization's integer-indefinite value and raises `FE_INVALID`;
representable inputs still exercise CCC's ordinary C cast and exception path.

The deterministic level-1 seed exercises extended-precision addition,
subtraction, multiplication, and division at full 80-bit precision in the four
x87 rounding modes; signed/unsigned 64-bit and binary32/binary64 conversions;
quiet equality; and signaling ordered comparisons. TestFloat compares result
bits and floating-point exception flags across 894,816 weighted boundary and
random cases at each of `-O0` and `-O2`. Exact NaN payload matching remains off
because TestFloat documents that option as valid only when the subject's NaN
selection policy matches SoftFloat; quietness and required invalid exceptions
are still checked. A reported discrepancy is an oracle failure that requires
case-level interpretation, not an automatic claim about which implementation
is wrong.
Driver tests require ABI-changing `long double` mode options to fail before
translation; no partial object is emitted.

TLS tests inspect `.tdata`/`.tbss` or Darwin `__thread_*` sections, symbol type
and binding, and the exact target relocation families. The required families
are TLSGD/TLSLD/DTPOFF/GOTTPOFF/TPOFF on x86-64, TLSDESC/TLSIE/TLSLE on
AArch64 Linux, TLS GD/TLS GOT/TPREL on RISC-V, and TLV page/page-offset pairs on
Darwin arm64. Each source model links and executes in a default PIE. A pthread
fixture proves distinct addresses and initializer values per thread for
external and block-local TLS, while two-direction reference-compiler links
verify TLS symbol interoperability. Generated accessors must retain unwind
information, a non-executable stack note on ELF, deterministic manifest
ownership, target symbol spelling, and exact local binding after packaging.

Planner, IR, digest, renderer, fake-command, and manifest tests run on every
host. Native TLS execution runs on every enabled host profile. The broader
x86-specific ABI and object suites compile only on Linux x86-64, reject the
wrong host at compile time, preflight every reference and packaging tool, and
invoke each named native test binary explicitly. A missing tool or zero-test
configuration is a hard failure, never an implicit skip.

The standalone target-oracle harness uses the same fail-closed rule. AArch64
Linux and RISC-V Linux run two-way CCC/reference-compiler fixed, variadic, and
TLS calls at `-O0` and `-O2`, inspect the resulting ELF objects, and exercise
static CFI through `_Unwind_Backtrace`. Native target hosts use GDB; cross hosts
execute through QEMU and attach `gdb-multiarch` to its gdbstub.
Both optimization profiles also execute returns-twice control flow and native
1-, 2-, 4-, and 8-byte scalar atomics; the atomic object must not import a
generic `__atomic_*` or `__sync_*` library entry point.
Darwin arm64 runs the equivalent native matrix with Apple Clang, Mach-O object
inspection, libunwind, and LLDB. Each runner records its compiler, sysroot or
SDK, emulator/debugger, deployment target, and linker identities. Header and
predefined-macro sentinels must agree with the selected profile. The corpus
harness reports an explicit applicable, inapplicable-with-reason, or failed
result for every case; an empty applicable set, absent runner, or missing tool
fails the harness invocation.

## Runtime-sized automatic storage

Provider-independent tests first prove that nonconstant array extents are
evaluated once and retained in the typed AST for parameters, declarations,
typedefs, and evaluated type names, including parameter-order binding and
multidimensional pointer strides. Diagnostics distinguish prototype-scope
`[*]`, legal fixed-size objects with variably modified types, nonautomatic VLA
objects, invalid bounds, and illegal storage classes. Named-goto and switch
tests reject ingress that bypasses a declaration, and a computed goto is
rejected conservatively in any function that also has a variably modified
declaration.

CCC-IR tests pin runtime storage identities, allocation effects, retained
extents, explicit checked `RuntimeSize` operations, runtime `sizeof`, dynamic
pointer strides, verifier type/dominance checks, and append-only digest tags.
Execution fixtures cover bound-once evaluation, multidimensional access,
normal return cleanup, recursion, concurrent invocations, and over-alignment.
Aggregate returns sourced from arena storage are copied before the provider is
released.

Provider tests inject nonpositive bounds, extent and alignment overflow,
allocation failure, and alignments through over-aligned `_Alignas` declarations.
On System V AMD64 every VLA allocation is checked for the target's 16-byte
minimum as well as stronger declared alignment. An instrumented descending-size
loop proves that one growth serves every later execution of the declaration and
that normal return releases it once. An external GCC default-PIE link runs the
same provider surface under AddressSanitizer and LeakSanitizer. Every enabled
native host profile executes the nonpositive, overflow, and provider-failure
traps. Object inspection additionally verifies the System V AMD64 `ud2`
failure paths when a development host's amd64 emulator cannot deliver those
trap signals correctly.

Returns-twice and cross-language unwinding remain separate hard gates. A
nonlocal exit may strand the abandoned invocation's cached allocations, as C
permits for VLA storage, but cannot share or corrupt another invocation's state.
The provider is not async-signal-safe because growth calls the hosted allocator.

Object and disassembly checks prove that affected user functions keep their
ordinary Cranelift frame and import only the declared hosted dependencies. A
mixed-link test links a CCC-produced object with an external GCC-compatible
driver as PIE. Runtime layout without an automatic object, such as VLA
`sizeof`, is checked separately and must not import the allocation provider.
The target-oracle runner compiles, links against the real hosted provider, and
executes the VLA bound/`sizeof` fixture and its provider-failure companion at
`-O0` and `-O2` for every enabled target. Negative feature-predicate tests keep
`__builtin_alloca` unavailable for an arena-only profile.

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
operation. Focused linker-model fixtures cover ordered and grouped archive
extraction, `-L`/`-l`, forced undefined and entry symbols, COMMON and weak
precedence, thin members, dynamic-library state, startup-file suppression, and
isolation of the compiler-runtime
provider from user whole-archive state. LLVM 18 and 19 have a documented x86-64 ABI bug that keeps `%r9`
reserved after atomically spilling an `__int128` argument
([LLVM #123935](https://github.com/llvm/llvm-project/issues/123935)). Those
versions still execute the spilled wide argument in both cross-link directions,
but the exact following-scalar placement is exercised with GCC and Clang 20 or
newer and locked independently by the ABI-plan assertion. No corpus result
substitutes for this matrix.

## Differential testing and undefined behavior

Differential tests compare only outputs whose relevant behavior is defined for the identical effective configuration.

- Csmith supplies generated defined-behavior programs; generator version, options, seed, and target assumptions are recorded.
- Handwritten differential tests include a short statement of the standard/extension rule that makes the observed result defined. Tests with unspecified evaluation order, padding bytes, pointer ordering outside one object, uninitialized data, data races, or implementation-defined settings not fixed by the configuration are excluded from output comparison.
- GCC and Clang are run at multiple optimization levels. UBSan and ASan runs are useful bug detectors but are not accepted as proof that a program is free of undefined or unspecified behavior.
- Metamorphic tests compare semantics-preserving variants without relying on a single reference compiler.
- C-Reduce reductions preserve an interestingness predicate that reruns all definedness checks; reduced fixtures retain provenance and license metadata.

## Target execution matrix

Hosted CI runs the Rust workspace test suite on native x86-64 Linux, AArch64
Linux, and AArch64 macOS runners. The RISC-V64 workspace tests are
cross-compiled with Rust's `riscv64gc-unknown-linux-gnu` target and executed
through QEMU user mode with the matching GNU sysroot. The same Rust suite is
required on every matrix row, followed by that row's matching target oracle.

The standalone ABI, target-oracle, differential, and exhaustive real-code
corpus harnesses remain available for focused local qualification. A separate
x86-64 Linux job runs the bounded SQLite, Lua, bzip2, zlib, Redis, and zstd
profiles. Dedicated AArch64 Linux, RISC-V Linux, and arm64 Darwin jobs run the
complete bzip2 target contract, including target-object inspection and the
upstream and extended execution suites. Compile-only object inspection
supplements execution but never substitutes for a claimed runnable target.

## Real-code corpus

The implemented SQLite, Lua, Redis, bzip2, zstd, zlib, and selected
libc-header gates exercise drop-in compatibility. Each integration
records the exact build command, enabled features, patches if any, expected
exclusions, and whether success means preprocess, compile, link, or run.
“Builds unmodified” is used
only when no source or build-system patch is applied.

musl, tcc, and c-testsuite remain catalog-only, non-blocking candidates. They
do not count as supported integrations until each has a pinned source and
hash, a target-applicability entry, a deterministic adapter, and an explicit
compile/link/run contract.

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
`testfixture` selects `__sync_synchronize` but no inline assembly, wide
integers, VLA objects, computed goto, or statement expressions. The `full` mode
keeps SQLite's `SQLITE_ENABLE_STMT_SCANSTATUS` fuzzcheck profile. Its one
generated `sqlite3.c` translation selects the volatile GNU x86-64 `rdtsc` form
with `=a` and `=d` outputs, which CCC retains explicitly and implements through
a deterministic hidden support routine. No source predicate is overridden.
The eight fuzzcheck support inputs, `alltest`, and `sessionfuzz` use the same
GNU C11 profile. The wrapper audits the effective language mode and strict-ANSI
state for every translation. Corpus
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
upstream Linux make target directly with `CC=ccc` and requires all 34 `.c`
files in the source directory to appear exactly once in CCC's source-input log.
The `CC=ccc` substitution replaces Lua's bundled `gcc -std=gnu99` command, so
CCC uses its documented GNU C default with no adapter language override. It
produces the exact 34-object inventory and 32-member archive, then drives both
final program links through the resolved target toolchain. Their normalized
arguments are checked against Lua's two upstream Linux link recipes. Those
links use the platform default without an adapter relocation flag, and the gate
verifies PIE ELF type `DYN` plus the absence of dynamic text relocations.

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
and an instrumented runtime.

Redis is pinned by its [corpus manifest](../../test-corpus/redis/manifest.toml)
to the official 8.8.0 core archive. The selected upstream build produces
`redis-server` and `redis-cli` with the libc allocator while disabling TLS,
systemd, optional vector sets, bundled data-type modules, and link-time
optimization. All 178 selected C translation units pass through CCC using its
GNU11 default; native GCC receives only the resulting objects and archives for
two platform-default PIE links. The wrapper filters upstream C99, GNU99, and
GNU11 selections rather than injecting a replacement standard flag. It mirrors
each real translation with a preprocessing
pass under the same effective language, macro, include, warning, and
optimization arguments. The adapter requires exactly 178 nonempty captures,
compares their relative paths to the pinned source set, and records exact
expanded-builtin counts. It also audits the complete compiled source multiset,
compiler identity, link inputs, ELF executable type, and absence of dynamic
text relocations. Native `long double` arithmetic and conversions use localized
x87 helpers, while generated System V bridges preserve f80 fixed, variadic,
indirect, aggregate, and `va_arg` boundaries to internal code and libc.

Redis assertions remain enabled through the unmodified system header; CCC
implements its GNU statement expression and `__PRETTY_FUNCTION__` surface.
The hosted `math.h` wrapper supplies single-evaluation binary64 classification
macros without exposing unselected native-`long double` arms. Redis's two
compiler barriers, four locked atomic statements, and one xxHash read/write
register guard reach CCC unchanged. The adapter audits their exact expansion
counts and rejects any neighboring inline-assembly form.

The Redis execution profile starts the CCC-built server on a private Unix
domain socket and drives `PING`, string, counter, list, hash, Lua `EVAL`, and
database-size checks through the CCC-built client. It is deliberately a
focused build-and-smoke contract: the upstream full test suite is not invoked.

bzip2 is pinned by its [corpus manifest](../../test-corpus/bzip2/manifest.toml)
to the official 1.0.8 archive and a full commit and tree from the official
`bzip2-tests` repository. The upstream Makefile selects nine of the archive's
13 C files to build `libbz2.a`, `bzip2`, and `bzip2recover`; CCC must compile
each selected input exactly once, and native GCC performs only the two
source-free platform-default PIE program links. The build inventory also proves
that the four developer utilities were not silently substituted into the
product set.

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

The zstd adapter uses the upstream `ZSTD_NO_ASM=1` switch to exclude the
stand-alone amd64 translation unit while compiling the selected inline forms
unchanged. They comprise CPUID, an empty compiler barrier, a
conditional move, and `nop`/`.p2align` layout hints. Its unmodified dependency
and system assertion headers use CCC's native memory builtins, GNU statement
expressions, and function-name aliases. Native links use the platform PIE
default without a relocation flag. Upstream's bounded quick smoke target covers
compression, decompression, streaming, dictionaries, file handling, corruption
rejection, sparse files, and the selected threaded path; deterministic file and
stream round trips add byte-for-byte checks. Long-running fuzz and stress
profiles are not part of this gate.

zlib is pinned by its [corpus manifest](../../test-corpus/zlib/manifest.toml)
to the official 1.3.2 release archive. Its configure script and generated
Makefile run without a compiler wrapper or source adjustment: CCC performs all
core, example, shared-object, archive, and executable compile/link commands.
The gate compares the exact 34-entry C source multiset, runs upstream's static,
shared, and large-file tests, inspects PIE and shared ELF metadata, and performs
an independent byte-for-byte `minigzip` round trip. The default x86-64 release
uses checked-in lookup tables, so optional atomic table initialization remains
covered by focused compiler tests rather than inferred from this integration.

A curated execute-only compiler torture subset may supplement focused fixtures.
It is fetched rather than vendored, and its corpus manifest records the exact
upstream revision, selected paths, license, hashes, required language profile,
and exclusions for undefined or implementation-specific behavior. Passing that
subset does not replace exact typed-AST, IR, ABI, and execution regressions for
the supported constructs.

## Licensing, pinning, and supply-chain policy

- Csmith uses its BSD-style license; C-Reduce uses the University of Illinois/NCSA-style license; GCC tests are governed by the applicable GCC GPL terms. Each corpus/tool is recorded individually instead of being grouped under one license label.
- Only compatible, small fixtures are vendored. External corpora are fetched by immutable revision and cryptographic hash into a local cache or project-controlled mirror.
- Fetch scripts verify hashes and licenses before use; a network outage cannot silently select a different revision.
- Minimized fixtures derived from third-party tests retain the source license and provenance when the license requires it.
- External GPL tools may be executed without being linked into CCC; source-copying and distribution decisions are handled separately from tool execution.

Hosted CI runs the workspace Rust tests on every supported host profile, using
QEMU for RISC-V64, plus the bounded x86-64 real-code corpus job. Fuzzing,
differential matrices, full corpus modes, target oracles, and exhaustive ABI
shapes are explicit local qualification commands; their harnesses retain
failures as reproducible seeds and artifacts.
