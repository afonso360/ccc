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
runtime `sizeof`, and multidimensional pointer strides. Diagnostics distinguish
prototype-scope `[*]`, legal fixed-size objects with variably modified types,
runtime-sized objects whose provider is unavailable, invalid bounds, illegal
storage classes, and control-flow ingress that bypasses a declaration.

Enabling an automatic-storage provider additionally requires exact CCC-IR
goldens for region entry, allocation, and restoration, plus verifier mutations
for mismatched merges, address-before-entry, non-LIFO restoration, and return
with active storage. Execution fixtures cover normal fallthrough, nested
blocks, `break`, `continue`, outward and backward `goto`, return-value
evaluation, switch ingress, computed-goto cleanup when both capabilities are
enabled, recursion, concurrent invocations, and statement-expression lifetime.

Provider tests inject nonpositive bounds, extent and alignment overflow,
allocation failure, and alignments through over-aligned `_Alignas` declarations.
On System V AMD64 every VLA allocation is checked for the target's 16-byte
minimum as well as stronger declared alignment. Long-running loop fixtures
measure bounded high-water reuse: after restoration, an allocation that fits
retained capacity must make zero further allocator calls. Each target provider
descriptor commits maximum allocation-count, hot-call latency-regression, and
code-size budgets together with the pinned runner class, sampling protocol, and
tolerance; the benchmark is a failing gate rather than an informational
recording. Allocator accounting and LeakSanitizer runs cover every ordinary exit
shape.

The scoped-arena profile has explicit nonlocal-control tests. A same-function
combination with a returns-twice call remains a compile-fail case until its
checkpoint protocol is verified. A cross-invocation `longjmp` fixture abandons
one invocation while another arena is active and proves that the surviving
arena remains intact. The harness reports the abandoned invocation's expected
unreclaimed bytes separately so only that specified `longjmp` loss is exempt
from leak-freedom assertions. Configuration snapshots pin the provider's
negative async-signal-safety and cross-language-unwind facts; a call that may
unwind across active arena storage remains diagnosed until cleanup integration
is proved.

Object and disassembly checks prove that affected user functions keep their
ordinary Cranelift frame, support definitions have local binding, and the only
external provider references match the runtime manifest. A mixed-link test
links a CCC-produced object with an external GCC- and Clang-compatible driver
and resolves only the declared hosted dependencies. Negative feature-predicate
tests keep `__builtin_alloca` unavailable for an arena-only profile.

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

Computed-goto fixtures include a positive direct `&&label` pointer table and
exact `br_table` golden. Negative fixtures diagnose both label subtraction and
base-label-plus-offset reconstruction before lowering erases label provenance.

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

SQLite, Lua, zlib, musl, tcc, selected libc-header fixtures, and c-testsuite
exercise drop-in compatibility. Each integration records the exact build
command, enabled features, patches if any, expected exclusions, and whether
success means preprocess, compile, link, or run. “Builds unmodified” is used
only when no source or build-system patch is applied.

Hosted-header preprocessing and parsing are separate gates. A licensed, pinned
glibc-like fixture has both a deterministic preprocessing golden and an AST
surface check. On x86-64 Linux, installed glibc feature, definition, integer,
type, unistd, and string headers are also exercised by separate preprocessing
and parsing tests. The installed-header tests record compiler, target, and libc
identity and assert stable sentinel properties rather than snapshotting a
mutable system header. Parsing success certifies only the advertised parsing
ceiling; it does not imply semantic analysis or object-emission support for
parse-only declarations.

Corpus pins encode capability dependencies. SQLite is pinned by its
[corpus manifest](../../test-corpus/sqlite/manifest.toml) to 3.47.2. Release
3.47.0 removed SQLite's remaining `long double` use by switching that
calculation to Dekker's algorithm; an earlier release silently requires the f80
runtime on the primary target. This pin is also the last canonical-source
release before SQLite replaced the classic Autoconf configure/Makefile
interface with Autosetup in 3.48.0; any later pin requires an adapter-interface
review as well as a C-surface inventory. Its gate runs the `veryquick` Tcl set
through `testfixture`, which needs Tcl and zlib development environments; the
full suite and TH3 are out of scope. Under CCC's effective identity the build
selects `__sync_synchronize` but no inline assembly, wide integers, VLA objects,
computed goto, or statement expressions. Its success is integration evidence
rather than proof of the unselected constructs; their focused fixtures remain
required.
Lua's default GNU-profile build exercises `setjmp`/`longjmp` and computed-goto
dispatch. musl feeds `$CC` assembly files.

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
