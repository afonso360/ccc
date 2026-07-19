# Pipeline & crates

The annotated compilation pipeline, the per-component design, and the workspace layout. The high-level overview is in [`../ARCHITECTURE.md`](../ARCHITECTURE.md).

## Pipeline

C distinguishes **preprocessing tokens** (pp-tokens: pp-numbers, header-names, un-interpreted punctuators) from the **parser tokens** the grammar consumes (keywords, typed constants, concatenated string literals). Macro expansion operates on pp-tokens; the conversion to parser tokens is translation phase 7. The pipeline names both explicitly:

```
 source bytes (.c)
      │
      ▼
  Driver (ccc)  ──────────────── GCC/Clang-compatible CLI, phase orchestration    [ccc-driver]
      │
      ▼
  pp-token lexer  ─────────────── phases 1–3: encoding, line splicing, pp-tokens   [ccc-pp]
      │  (pp-token stream, with provenance)
      ▼
  Preprocessor  ──────────────── phase 4: macro expansion, #include, #if, #pragma  [ccc-pp]
      │  (expanded pp-token stream)
      ▼
  Token conversion  ──────────── phases 5–7: keywords, literal decode, string      [ccc-syntax]
      │  (parser-token stream)              concatenation, pp-number → constant
      ▼
  Parser  ◀──────── syntax-owned NameClassEnv + recorded scope events               [ccc-syntax]
      │  (untyped AST)
      ▼
  Semantic analysis  ─────────── scopes, full type system, conversions, const-eval [ccc-sema]
      │  (typed AST — every implicit conversion explicit)
      ▼
  IR lowering  ───────────────── desugar → CCC-IR (typed, ABI-independent, CFG)    [ccc-ir]
      │  (CCC-IR)
      ▼
  ABI planning  ──────────────── per-target classification, sret, vararg bridges  [ccc-abi]
      │  (CCC-IR + immutable ModuleAbiPlan)
      ▼
  Codegen  ───────────────────── cranelift-frontend FunctionBuilder → CLIF         [ccc-codegen]
      │
      ▼
  Object + debug emission  ───── cranelift-object + CCC/gimli DWARF → .o
      │
      ▼
  Target tools  ───────────────── resolved assembler/linker/archiver + bridges      [ccc-link]
      │
      ▼
  executable / shared library / archive
```

## Component breakdown

**Driver (`ccc-driver`).** CLI, phase selection, immutable effective configuration, resource/toolchain resolution, and link-plan orchestration. Full behavior is in [Driver & CLI](driver-cli.md).

**Preprocessor (`ccc-pp`).** Owns translation phases 1–4: encoding, trigraph/line-splice handling, pp-token formation, macro expansion, include resolution, conditionals, pragmas, feature predicates, and predefined macros. A pp-token retains spelling, leading-space/start-of-line state, source and macro provenance, and expansion-suppression/hide-set state; token pasting also represents placemarkers explicitly. Feature predicates query the shared GNU/capability registry. `ccc-pp` also owns `#if`/`#elif` constant-expression evaluation — it is phase 4: `intmax_t`/`uintmax_t` arithmetic, `defined`, feature predicates, unknown identifiers as zero — and therefore the single pp-number/character-constant decoder behind it; `ccc-syntax` reuses that decoder in phase 7 so the preprocessor and the parser cannot disagree on the same numeric or character spelling. Depfile generation is driven from the same include resolver described in [Resource directory](resource-dir.md). Owning pp-token lexing is [ADR-0005](../adr/0005-preprocessor-owns-pp-token-lexing.md).

**Token conversion + Parser (`ccc-syntax`).** Owns phases 5–7 (escape/charset decode, adjacent string-literal concatenation, pp-number → typed constant applying target typing rules on top of the shared `ccc-pp` decoder, keyword recognition) and the parser. Hand-written **recursive descent** ([ADR-0004](../adr/0004-recursive-descent-parser.md)) for diagnostic quality and context-sensitivity.

_Typedef-name classification, concretely ([ADR-0002](../adr/0002-syntax-owned-typedef-classification.md)):_ `ccc-syntax::NameClassEnv` implements C's ordinary-identifier namespace, scope kinds, shadowing, and exact point-of-declaration events. The parser consults it to disambiguate `T * x;` without calling semantic analysis. The AST records those scope/binding events; `ccc-sema` replays and validates the same event model instead of maintaining a divergent second interpretation.

**Semantic analysis (`ccc-sema`).** The heaviest correctness component: the full C type system (integer promotions, usual arithmetic conversions, array/function decay, qualifiers, tag/typedef namespaces, block/function/file scoping), lvalue rules, and the constant-expression evaluator (`case` labels, array sizes, `_Static_assert`, initializers, bitfield widths, `enum` values). Output is a fully typed AST with **every implicit conversion made explicit** — codegen is never left to infer one. Type representation and layout live in the shared `ccc-types` crate ([ADR-0009](../adr/0009-shared-type-and-layout-crate.md)): sema constructs and interns types there, and the typed AST, CCC-IR, `ccc-abi`, and codegen all reference the same interned types and one layout engine. The guarantees it hands downstream are listed in [CCC-IR invariants](ccc-ir.md).

**CCC-IR (`ccc-ir`, locked — [ADR-0001](../adr/0001-ccc-ir-middle-layer.md)).** A typed, ABI-independent mid-level IR in CFG form between the typed AST and Cranelift. All surface-language desugaring happens here once (loops → blocks+branches, `&&`/`||`/`?:` → control flow, compound assignment expanded, implicit conversions materialized). It hosts backend-independent C-level optimizations and retains explicit memory/call effects. Its semantic contract is [CCC-IR invariants](ccc-ir.md).

**ABI planning (`ccc-abi`).** Produces one immutable target-specific
[module ABI plan](abi-and-varargs.md#module-abi-plan) for definitions and call
sites: aggregate classification, native carrier encoding, bridge placement,
hidden returns, extensions, variadic shaping, and packaging requirements. The
plan remains separate from CCC-IR, is verified against a canonical IR digest,
and is tested against the [ABI oracle](testing.md#abi-oracle).

**Codegen (`ccc-codegen`).** Uses `cranelift-frontend`'s `FunctionBuilder`; eligible scalar locals become Cranelift `Variable`s while memory-resident objects follow the CCC-IR place/effect rules. It consumes the verified `ModuleAbiPlan`, checks backend capabilities, and emits CLIF or a hard diagnostic—never an ABI approximation. Provider-neutral runtime-sized automatic storage may lower to versioned local CLIF support definitions in the primary object; this is distinct from generated target assembly. With source debugging enabled, exact CCC spans become backend source identities and a separate gimli layer emits line, type, scope, variable, and relocation data after machine layout is known; System V call-frame information remains independently emitted. `cranelift-object` emits the primary object; generated bridge/assembly inputs are owned by `ccc-link`.

## Workspace layout

A Cargo workspace decomposed along the IR boundaries (and the pp-token/parser-token split above), so crates stay independently testable and acyclic:

```
ccc/
├─ crates/
│  ├─ ccc-driver/      # `ccc` binary: CLI, flag policy, phase orchestration, resource dir, linking
│  ├─ ccc-session/     # SourceManager, spans, provenance, compilation-unit config
│  ├─ ccc-diag/        # diagnostics: rendering, codes, categories, macro/include backtraces
│  ├─ ccc-target/      # target defaults + the `EffectiveCompilationConfig` type (constructed by `ccc-driver`)
│  ├─ ccc-types/       # canonical C type representation + layout engine (shared by sema/IR/ABI/codegen)
│  ├─ ccc-pp/          # phases 1–4: pp-token lexer + preprocessor + `#if` eval + include/depfile resolver
│  ├─ ccc-syntax/      # phases 5–7: token conversion + parser + AST + parser-owned typedef table
│  ├─ ccc-sema/        # type system, scopes, const-eval → typed AST
│  ├─ ccc-ir/          # CCC-IR definition + builder + invariants + C-level opt passes
│  ├─ ccc-abi/         # module ABI planning + target-specific boundary classification
│  ├─ ccc-codegen/     # CCC-IR → CLIF via cranelift-frontend/object
│  └─ ccc-link/        # resolved target tools, archives, bridge/asm objects, final link plan
├─ resource-dir/       # shipped builtin headers (stdarg.h, stddef.h, …) + runtime shim
├─ tests/              # integration: execution, differential, ABI-oracle, compile-fail
├─ test-corpus/        # manifests, compatible fixtures, pins, hashes, and fetch metadata
└─ docs/
```

Dependency direction is acyclic: `ccc-syntax` depends on `ccc-pp` (pp-token definitions and the shared literal decoder) and on `ccc-target`/`ccc-diag`/`ccc-session`, but not `ccc-sema`; `ccc-sema` depends on syntax and reuses its scope-event definitions; `ccc-types` sits above `ccc-target` and below `ccc-sema`, `ccc-ir`, `ccc-abi`, and `ccc-codegen`, which share its type table and layout engine ([ADR-0009](../adr/0009-shared-type-and-layout-crate.md)). Shared configuration is immutable and does not create callbacks from lower layers into the driver.
