# Testing and correctness strategy

Correctness is checked at each explicit compiler boundary and at binary interfaces with independent compilers.

## Test tiers

- **Unit/property tests:** token formation, macro expansion, declarators, type/layout rules, constant evaluation, IR invariants, target configuration, and ABI classifiers.
- **Snapshot tests:** deterministic preprocessed output, tokens, AST, typed AST, CCC-IR, ABI plans, CLIF, diagnostics, effective configuration, and link plans.
- **Execution tests:** compile, link, run, and assert exit status/stdout/stderr for every executable target environment.
- **Compile-fail tests:** stable diagnostic codes, primary spans, macro/include provenance, and essential wording; incidental formatting is normalized.
- **Object/disassembly tests:** sections, symbols, visibility, relocations, TLS, DWARF, calling-sequence details, PIE/PIC behavior, and generated bridges.

## ABI oracle

For every target:

- generated classifier tests compare CCC layout and call classification with the published psABI and independently inspect GCC/Clang output;
- cross-linked tests compile CCC callers with reference callees and reference callers with CCC callees;
- aggregate, hidden return, register exhaustion, stack alignment, variadic, TLS, long-double, and helper-call cases are included;
- matching CCC on both sides is never treated as proof of platform ABI compatibility.

Variadic tests separately cover call sites and definitions, direct and indirect calls, promoted floating-point arguments, aggregates, register/stack boundaries, `va_copy`, and target-specific state such as SysV `%al`. Long-double tests verify size/alignment/macros, arithmetic, object representation, and calls in both directions; explicit compatibility-mode objects must be rejected when mixed with incompatible CCC objects.

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

SQLite, Lua, zlib, musl, tcc, selected libc-header fixtures, c-testsuite, and GCC torture tests exercise drop-in compatibility. Each integration records the exact build command, enabled features, patches if any, expected exclusions, and whether success means preprocess, compile, link, or run. “Builds unmodified” is used only when no source or build-system patch is applied.

Corpus pins encode capability dependencies. SQLite is pinned to ≥ 3.45, which removed core `long double` arithmetic — an earlier pin silently requires the f80 runtime on the primary target — and its gate runs the `veryquick` TCL set through `testfixture`, which needs a TCL development environment; the full suite and TH3 are out of scope. Lua's default GNU-profile build exercises `setjmp`/`longjmp` and computed-goto dispatch. musl feeds `$CC` assembly files.

## Licensing, pinning, and supply-chain policy

- Csmith uses its BSD-style license; C-Reduce uses the University of Illinois/NCSA-style license; GCC tests are governed by the applicable GCC GPL terms. Each corpus/tool is recorded individually instead of being grouped under one license label.
- Only compatible, small fixtures are vendored. External corpora are fetched by immutable revision and cryptographic hash into a CI cache or project-controlled mirror.
- Fetch scripts verify hashes and licenses before use; a network outage cannot silently select a different revision.
- Minimized fixtures derived from third-party tests retain the source license and provenance when the license requires it.
- External GPL tools may be executed without being linked into CCC; source-copying and distribution decisions are handled separately from tool execution.

Fast unit/snapshot/execution tiers run per change. Fuzzing, differential matrices, full corpora, QEMU, native Darwin, and exhaustive ABI shapes run on scheduled and release CI, with failures retained as reproducible seeds/artifacts.
