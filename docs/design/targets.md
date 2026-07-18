# Targets and non-goals

Targets are named by exact triples. A target is enabled only when its object format, ABI classifier, runtime helpers, toolchain resolver, and execution-test environment satisfy the same capability manifest.

| Internal triple               | User-facing form                                      | Object | ABI / calling convention | libc       | Target defaults                                                                                |
| ----------------------------- | ----------------------------------------------------- | ------ | ------------------------ | ---------- | ---------------------------------------------------------------------------------------------- |
| `x86_64-unknown-linux-gnu`    | same                                                  | ELF    | System V AMD64, LP64     | glibc      | signed `char`; f80 `long double`                                                               |
| `x86_64-unknown-linux-musl`   | same                                                  | ELF    | System V AMD64, LP64     | musl       | signed `char`; f80 `long double`; static linking available when the musl toolchain is resolved |
| `aarch64-unknown-linux-gnu`   | same                                                  | ELF    | AAPCS64, LP64            | glibc      | unsigned `char`; HFA/HVA; binary128 `long double`                                              |
| `riscv64gc-unknown-linux-gnu` | `riscv64-unknown-linux-gnu -march=rv64gc -mabi=lp64d` | ELF    | RISC-V LP64D             | glibc      | unsigned `char`; FP aggregates; binary128 `long double`                                        |
| `aarch64-apple-darwin`        | same, plus deployment target                          | Mach-O | Darwin arm64             | Apple libc | signed `char`; Apple variadics; `long double == double`                                        |

Internal triples use target-lexicon canonical forms. The driver normalizes a user triple plus `-march`, `-mcpu`, `-mabi`, deployment-target, and feature flags into an effective configuration. It rejects contradictory combinations instead of silently discarding an option.

## Effective compilation configuration

The `EffectiveCompilationConfig` type and per-target defaults are defined in `ccc-target`; `ccc-driver` constructs one immutable value before preprocessing, and `ccc-session` carries it so every compiler phase receives the same value or a stable ID for it. It contains:

- `TargetSpec`: triple defaults, data layout, object format, default ABI, default CPU features, and native `long double` representation;
- `LanguageOptions`: language/GNU profile, character set, overflow, enum, character-signedness, and diagnostic-affecting choices;
- `AbiOptions`: calling convention, packing, long-double mode, vector ABI, TLS model, and target ABI flags;
- `CodegenOptions`: CPU/ISA features, relocation model, code model, optimization contract, debug information, stack policy, and automatic-storage provider;
- `ToolchainSpec`: resolved compiler driver, assembler, linker, archiver, sysroot/SDK, runtime libraries, system includes, deployment target, and a probe fingerprint. Components are resolved [per selected phase](toolchain.md#target-toolchain-resolution); compile-only invocations do not require a resolved linker.

Target defaults remain immutable data; command-line flags produce a new effective value rather than mutating global target state. Predefined macros, builtin headers, layout, semantic analysis, ABI lowering, code generation, and linker flags are derived from this effective value. It is hashed into caches and recorded in object metadata needed for compatibility checks; each resolved tool is fingerprinted individually and hashes cover the phase-relevant subset, so compile-only outputs do not depend on linker identity.

Target data layout is the source of truth for predefined type spellings,
integer limits and widths, and `__SIZEOF_*__` values. The frontend combines
those facts with the language and named compatibility profile once; the driver
adds only compiler identity and capability-denial macros. Builtin headers and
`-dM` consume that same final environment rather than maintaining parallel
tables.

An automatic-storage provider is enabled per effective target profile. Its
versioned descriptor records the provider kind, arena and mark record layouts,
allocator requirements, target VLA minimum alignment, failure behavior,
returns-twice and cross-language-unwind compatibility, async-signal-safety
stance, and committed performance budgets. Generated callers and local support
definitions consume the same record layouts. These facts drive semantic
diagnostics, `__STDC_NO_VLA__`, `__has_builtin`, CCC-IR lowering, helper
selection, link planning, and provider tests together. Arena-backed ISO VLA
support never implies native-stack builtin support. The descriptor revision and
record layouts enter both the effective-configuration hash and object
compatibility metadata.

## Relocation and output models

Executable, PIE, shared-library, and static-object modes select matching Cranelift relocation/code-model flags and linker-driver arguments. CCC never feeds non-PIC relocations into a driver invocation that defaults to PIE: an explicit non-PIE build passes the target driver's disable-PIE option, while a PIE build emits compatible code. Darwin shared output uses dylib conventions; ELF uses shared-object conventions.

Toolchain availability is part of target support. A host `cc` is not assumed to target a different triple. Darwin output requires a compatible Apple SDK and toolchain and runs on native macOS CI; Linux cross-target output uses its matching sysroot/toolchain and may execute under QEMU.

## Non-goals

The driver recognizes and clearly rejects these target families or modes:

- Windows MSVC/GNU ABIs, SEH, and Windows object formats;
- 32-bit, big-endian, and ILP32-on-64 targets;
- kernel, embedded, or freestanding profiles beyond the documented builtin-header contract;
- C++, Objective-C, exceptions, LTO, sanitizers, and profiling instrumentation.

Rejecting a non-goal happens before compilation and names the unsupported target or capability; it must never fall back to the host target.
