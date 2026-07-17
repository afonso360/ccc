# Core C11 and selected GNU semantics

This document defines the semantic and evidence contract for the C11 and GNU
constructs that extend CCC's existing scalar language surface. It is an
activation specification, not a claim that every listed capability is
currently available. Current boundary status remains authoritative in the
[frontend capability inventory](frontend-capabilities.md), and feature
predicates become true only after the implementation and evidence described
here are complete.

The contract is deliberately target-specific where representation, ABI, or
runtime helpers are observable. The initial evidence target is
`x86_64-unknown-linux-gnu` in GNU C11 mode.

## Effective compiler identity

The hosted GNU profile uses `__GNUC__ == 4`, `__GNUC_MINOR__ == 2`, and
`__GNUC_PATCHLEVEL__ == 1` to select a conservative header path. Those three
macros are a compatibility gate, not a claim that CCC reproduces the complete
historical GCC 4.2.1 preprocessor or language surface.

The effective identity is the union of the following independently truthful
facts:

| Identity component      | Contract                                                                                                                                                                                                                                                                  |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GNU compatibility tuple | Exactly `4.2.1`; changing it is a capability change because source can select different declarations, builtins, and assembly.                                                                                                                                             |
| Language identity       | `__STDC__ == 1` and `__STDC_VERSION__ == 201112L`; strict C11 additionally defines `__STRICT_ANSI__`.                                                                                                                                                                     |
| CCC identity            | `__CCC__` and the `__CCC_{MAJOR,MINOR,PATCHLEVEL}__` tuple identify the actual compiler.                                                                                                                                                                                  |
| Target facts            | Width, limit, byte-order, data-model, object-format, architecture, and operating-system macros come from the effective target configuration. They are not copied from the host compiler.                                                                                  |
| Capability denials      | `__STDC_NO_ATOMICS__`, `__STDC_NO_COMPLEX__`, `__STDC_NO_THREADS__`, and `__STDC_NO_VLA__` are derived from the registry and disappear only when the corresponding complete capability is active.                                                                         |
| Dynamic predicates      | `__has_include`, `__has_include_next`, `__has_attribute`, `__has_builtin`, `__has_feature`, and `__has_extension` are CCC-provided operators. Their existence is an intentional deviation from GCC 4.2.1, and each answer comes from the resolver or capability registry. |
| Dynamic counter         | `__COUNTER__` is provided even though it postdates GCC 4.2.1. Its monotonic translation-unit-local behavior is part of CCC's identity.                                                                                                                                    |
| Wide-integer fact       | `__SIZEOF_INT128__` is absent until the complete `__int128` syntax, arithmetic, conversion, layout, ABI, varargs, and helper-link contract is active. Defining it is another intentional deviation from GCC 4.2.1.                                                        |

This list is the baseline delta from the advertised GCC version. Adding another
operator or predefined macro that could select a source branch requires an
entry here, a configuration snapshot, and a new inventory run. Predicate
existence must eventually be represented in the effective profile rather than
remaining an unconditional preprocessor implementation detail. The resource
manifest version changes when that machine-readable identity surface changes.

`ccc -dM -E` is necessary but insufficient evidence because CCC's dynamic
predicate operators are considered defined without appearing in the macro
dump. Every inventory therefore records both the sorted macro dump and an
explicit probe covering predicate existence and answers. The probe includes
`-U` behavior, one supported and one unsupported registry entry per predicate
family, and source-relevant queries such as
`__has_extension(c_atomic) == 0`.

## Source capability inventory

Builtin and inline-assembly requirements are derived from the exact source,
configuration flags, and effective CCC identity used by a build adapter. A run
under host GCC or Clang is not inventory evidence: their version macros and
predicates can select a materially different program.

An inventory record contains:

- the corpus manifest and source hashes;
- the complete effective macro dump and explicit predicate probe;
- every translation command after build-system expansion;
- the preprocessed branch that admits each builtin or assembly statement;
- the exact spelling, type, side effects, constraints, clobbers, and required
  backend or runtime-helper behavior; and
- the corresponding capability-registry state and focused execution test.

The pinned SQLite configuration currently selects the following compiler
surface under CCC's identity:

| Source gate                                                                    | Selected behavior                                                                                                                              |
| ------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `GCC_VERSION >= 4001000` in `src/mutex_unix.c`                                 | `__sync_synchronize()` is selected. CCC must lower it as a compiler and hardware full barrier; it is not an ordinary external call or a no-op. |
| `GCC_VERSION >= 4003000` or `>= 4008000`                                       | Byte-swap builtins are not selected.                                                                                                           |
| `GCC_VERSION >= 4007000` or `__has_extension(c_atomic)`                        | `__atomic_load_n` and `__atomic_store_n` are not selected because the version test and predicate are both false.                               |
| `GCC_VERSION >= 5004000`                                                       | Overflow and count-leading-zero builtins are not selected.                                                                                     |
| `VDBE_PROFILE`, `SQLITE_PERFORMANCE_TRACE`, or `SQLITE_ENABLE_STMT_SCANSTATUS` | None is enabled by the pinned test adapter, so `src/hwtime.h` contributes no inline assembly.                                                  |

The builtin registry admits `__sync_synchronize` independently and does not
infer support for any other `__sync_*`, `__atomic_*`, byte-swap, overflow, or
bit-count spelling. If a later adapter selects the hardware-timing header, the
first candidate to certify is the x86-64 volatile `rdtsc` form with `=a` and
`=d` outputs. No general inline-assembly certifier is built merely in
anticipation of that change.

## ISO C11 semantics

### Static assertions

`_Static_assert` is valid at file, record-member, and block scope. Its first
operand is an integer constant expression and a zero result is a diagnostic.
Strict C11 requires the string-literal message. GNU C11 may accept the
message-less later-standard spelling as an explicitly tested extension, but
that acceptance does not leak into strict mode.

### Function non-returning contract

`_Noreturn` applies only to a function declaration or definition and remains a
property of all compatible redeclarations. A direct call to a known non-returning
function terminates its CCC-IR block with `unreachable`; the verifier rejects a
fallthrough edge. Returning from a declared non-returning definition is
diagnosed where statically evident and otherwise retains the language's
undefined-behavior contract. The specifier does not imply `inline`, `nothrow`,
or any calling-convention change.

### Alignment specifiers

`_Alignas(type-name)` and `_Alignas(constant-expression)` are accepted only in
the declaration contexts C11 permits. Zero has no effect. A nonzero requested
alignment must be a valid target alignment and cannot weaken the natural
alignment; multiple specifiers combine to the strictest request. The effective
alignment is shared by type layout, object layout, symbol emission, ABI
classification, configuration digests, and `_Alignof`. Parameters, typedefs,
bit-fields, function declarations, and other forbidden subjects receive a
semantic diagnostic rather than having the specifier ignored.

### Generic selection

The controlling expression is not evaluated. Lvalue, array-to-pointer, and
function-to-pointer conversions are applied for compatible-type selection in
accordance with the C17 resolution of WG14 issue 481. Exactly one association
must match after those conversions, unless a `default` association supplies the
result.

Only the selected association is evaluated. The generic selection then has the
selected expression's type, value, and value category; in particular, a
selected lvalue remains an lvalue. This result modeling is independent from GNU
statement-expression result modeling.

### Compound literals

A compound literal creates one unnamed object with the specified type and
initializer. File-scope occurrences have static storage duration; block-scope
occurrences have automatic storage duration associated with the enclosing
block. The expression is an lvalue designating that object. Initializer
checking, zero-fill, relocations, qualifiers, volatile access, and aggregate
copy reuse the ordinary object-initialization and place rules. Array bound
completion and nested designators are tested explicitly.

### Flexible array members

A flexible array member is an incomplete array that appears last in an
otherwise nonempty structure as C11 permits. Its offset and element alignment
participate in layout, but `sizeof` the structure excludes element storage.
The member cannot be initialized as ordinary in-object elements, used in a
union, followed by another member, or silently treated as a zero- or one-element
array. Member access uses the existing layout projection and is valid only when
the containing object actually has sufficient trailing storage.

### Variably modified types

Expression bounds, prototype `[*]`, parameter adjustment, runtime `sizeof`,
and multidimensional strides follow the provider-independent type contract in
[the conformance policy](conformance.md#variable-length-arrays). Physical
runtime-sized object storage remains separately gated by
[ADR-0011](../adr/0011-arena-backed-runtime-sized-automatic-storage.md). Type
support neither implies native-stack mutation nor enables GNU `alloca`.

## GNU C semantics

### Statement expressions

`({ ... })` introduces a block scope and evaluates as one non-interleaved
subexpression of its containing expression. After GCC-compatible treatment of
trailing null statements, a body with no retained final expression has type
`void`. Otherwise the final expression provides the result after all preceding
block items have executed.

GNU C has three observable result categories, and CCC preserves them
explicitly:

- `void` for a body without a final expression statement;
- an ordinary non-lvalue value for a scoped or sequenced body whose result must
  be captured before leaving the block; and
- a transparent lvalue when, after empty statements are discarded, the body
  contains only one eligible final scalar, aggregate, or bit-field lvalue
  expression and contains no declarations. Its place retains qualifiers
  and any bit-field descriptor. Array and function results still undergo decay.
  CCC does not reproduce GCC's inconsistent generalized-lvalue bugs for casts
  or other expressions that are not C lvalues.

The transparent case is covered by assignment, address, aggregate-member, and
bit-field probes, including preservation of `const`. Bodies containing
declarations or earlier statements are covered separately and must materialize
a non-lvalue result. These are the GCC C frontend's observable rules; the GCC
manual's explicit returned-by-value wording applies to G++, not GNU C. These
rules do not reuse `_Generic`'s selected-expression value-category path.

Jumping into a statement expression is rejected. A `case` or `default` outside
the expression cannot target a label inside it, and a computed jump into it has
undefined behavior rather than a supported ingress edge. Jumping out follows
the containing expression's sequencing constraints and performs all required
scope cleanup. A VLA declared inside the expression is restored after the final
result is captured, so a pointer result does not extend the VLA's lifetime.

### Labels as values and computed goto

`&&label` produces a function-local opaque label token stored in a pointer-typed
value. Tokens may be copied, stored, compared for equality, placed in direct
pointer tables, selected by ordinary indexing, and consumed by
`goto *expression`. Lowering maps address-taken labels to per-function indices
and uses `br_table`; it never exposes machine code addresses.

Label provenance remains visible through semantic analysis. Arithmetic on a
label token is unsupported and receives a dedicated diagnostic, including the
position-independent difference-table idiom `&&target - &&base` and
reconstruction such as `&&base + offsets[i]`. No label-difference relocation is
emitted. A Lua-style table that stores the direct `&&label` tokens remains in
the supported subset. Cross-function use is undefined and diagnosed when
detectable.

### Wide integer extension

`__int128` and `unsigned __int128` are distinct 128-bit integer types with the
usual integer operations, conversions, qualifiers, pointers, arrays, and object
layout. Their built-in type IDs are appended after the existing built-in prefix;
the hand-assigned constants, `BuiltinType` ordering, and `TypeStore`
initialization order remain in lockstep and no existing ID is renumbered.

On System V AMD64, a wide integer boundary is classified as two `INTEGER`
eightbytes with whole-argument register rollback and the target-required memory
alignment. Calls, returns, variadic arguments, and mixed register pressure are
cross-linked in both directions with GCC and Clang. No other target advertises
the type until its own layout and ABI evidence exists.

Cranelift operations are used only where the pinned backend proves native
lowering. Wide division, remainder, and integer/floating conversions are
operation-sensitive compiler-emitted helper calls. The runtime-helper manifest
pre-budgets these symbol families:

- `__divti3`, `__udivti3`, `__modti3`, and `__umodti3`;
- `__floattisf`, `__floattidf`, `__floatuntisf`, and `__floatuntidf`; and
- `__fixsfti`, `__fixdfti`, `__fixunssfti`, and `__fixunsdfti`.

Every selected helper has an exact signature, target provider, object-symbol
test, link-plan entry, and cross-linked execution test. The backend's default
libcall-name table is not treated as support for helpers it does not model.

The SQLite build under the effective 4.2.1 gate does not select wide-integer
code and therefore supplies no evidence for this extension. Acceptance comes
from focused high-bit constants and arithmetic, conversion boundaries, object
layout, varargs, ABI pressure, and runtime-helper fixtures. Only after all of
those pass does the target define `__SIZEOF_INT128__`.

### Builtins and inline assembly

Each builtin spelling is an independent capability entry with an exact
signature, constant-expression rules, side effects, lowering, helper needs, and
feature-predicate answer. Family resemblance never admits another spelling.
Compiler barriers and hardware fences are represented as effects in CCC-IR so
optimization cannot move memory operations across them.

Inline assembly is retained losslessly through parsing: template, operands,
constraints and alternatives, ties, early-clobber markers, clobbers,
volatility, and goto labels. Emission is enabled only for an inventory-derived
form whose marshalling and register effects have been certified. An empty
corpus inventory causes no emission machinery to be built and leaves inline
assembly unavailable; it does not turn a parse-only construct into a no-op.

## Evidence partitions

The work is divided by technical dependency so one proof source cannot mask
another:

| Capability group                        | Required evidence                                                                                  | Relationship to SQLite                                                                 |
| --------------------------------------- | -------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Predefined identity and inventory       | Macro dump, explicit predicate probe, selected-branch inventory, registry snapshot                 | Determines the actual program SQLite asks CCC to compile.                              |
| Corpus-selected builtins                | Exact CCC-IR effect, machine-code inspection, concurrency execution                                | SQLite directly requires `__sync_synchronize`.                                         |
| Standard frontend semantics             | Typed-AST and CCC-IR goldens, diagnostics, execution fixtures                                      | SQLite is integration evidence only where its selected sources exercise the construct. |
| Statement expressions and computed goto | Focused value-category, cleanup, provenance, and `br_table` fixtures                               | The pinned SQLite sources do not exercise them.                                        |
| Wide integers                           | Layout, arithmetic, conversion, helper, varargs, and two-way ABI oracle                            | SQLite supplies no evidence.                                                           |
| Inline assembly                         | Exact selected-form inventory and a form-specific certifier                                        | The pinned adapter's inventory is empty.                                               |
| Corpus adapter                          | Source/hash verification, deterministic configure surface, build transcript, `veryquick` execution | Proves integration but does not replace any focused fixture above.                     |

The corpus adapter may be developed and run as soon as its selected surface is
available. Independent completeness capabilities retain their own hard gates
and cannot use corpus success as substitute evidence.

## References

- [C11 committee draft N1570](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1570.pdf)
- [WG14 issue 481, controlling expression of generic selection](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n2243.htm#dr_481)
- [GCC statement expressions](https://gcc.gnu.org/onlinedocs/gcc/Statement-Exprs.html)
- [GCC C frontend statement-expression lowering](https://github.com/gcc-mirror/gcc/blob/master/gcc/c/c-typeck.cc)
- [GCC bug 88773, cast result incorrectly treated as an lvalue](https://gcc.gnu.org/pipermail/gcc-bugs/2019-January/648755.html)
- [GCC labels as values](https://gcc.gnu.org/onlinedocs/gcc/Labels-as-Values.html)
- [GCC 4.3 changes](https://gcc.gnu.org/gcc-4.3/changes.html)
- [GCC 4.6 changes](https://gcc.gnu.org/gcc-4.6/changes.html)
- [GCC 10 changes](https://gcc.gnu.org/gcc-10/changes.html)
- [GCC integer runtime routines](https://gcc.gnu.org/onlinedocs/gccint/Integer-library-routines.html)
- [GCC floating conversion runtime routines](https://gcc.gnu.org/onlinedocs/gccint/Soft-float-library-routines.html)
