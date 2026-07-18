# Conformance policy

CCC targets pragmatic C11 plus a documented GNU compatibility profile. Every accepted construct has defined behavior; syntax that is recognized only for header compatibility remains represented in the AST and produces a hard diagnostic if semantic or code-generation support is required.

## Language modes

The default language mode is `gnu11`. The initially supported explicit modes are
`-std=gnu11` and `-std=c11`; other `-std=` values are rejected before reading an
input. Both modes define `__STDC_VERSION__` as `201112L`. Strict `c11` also
defines `__STRICT_ANSI__`. Trigraph replacement is enabled in strict `c11` and
by an explicit `-trigraphs` option, and disabled in `gnu11`; a disabled
trigraph that could change the program is covered by the `trigraphs` warning
category.

Input is UTF-8, with one optional leading UTF-8 byte-order mark. Universal
character names permitted in C11 identifiers are accepted and canonicalized
for identifier and macro-name equality while retaining their source spelling
for diagnostics and preprocessing output.

## `long double`

The default mode always preserves the selected target's C ABI, including representation, size, alignment, predefined macros, calling convention, and libc boundary behavior.

| Target                                                   | Native representation                                     | Required lowering                                                                                                                                        |
| -------------------------------------------------------- | --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `x86_64-unknown-linux-gnu` / musl                        | x87 extended precision, 80 value bits in a 16-byte object | Internal memory representation plus f80 helpers; assembly ABI bridges marshal x87 arguments/returns when Cranelift cannot express them.                  |
| `aarch64-unknown-linux-gnu`, `riscv64-unknown-linux-gnu` | IEEE binary128                                            | Exact object representation and aggregate layout; value-producing operations and ABI transport are rejected until independently proved.                 |
| `aarch64-apple-darwin`                                   | IEEE binary64, identical to `double`                      | Native Cranelift `f64`.                                                                                                                                  |

Declarations, `sizeof`, and `_Alignof` remain usable even when the selected backend lacks arithmetic or boundary support. Any literal conversion, arithmetic operation, call, return, or initializer that needs an unavailable capability is a hard, target-specific error. CCC must not substitute `double` implicitly.

`-mlong-double-64` is an explicit compatibility mode, never the default on a target whose ABI uses f80 or binary128. In that mode the [`EffectiveCompilationConfig`](targets.md#effective-compilation-configuration) changes the representation coherently: size, alignment, `__SIZEOF_LONG_DOUBLE__`, every `__LDBL_*__` macro, `<float.h>`, and ABI lowering all describe binary64. The driver emits one prominent ABI-incompatibility warning unless explicitly silenced. Objects produced in this mode carry a mode identifier in CCC metadata so the linker can diagnose incompatible CCC objects.

The runtime/helper and assembly-bridge availability is a target capability checked before code generation. Soft-float arithmetic alone is not considered ABI support.

The Linux binary128 profiles reject scalar and recursively containing
aggregate boundaries with `CCC3509`, variadic fetches with `CCC2404`, and
conversions, initialization, and arithmetic with `CCC2343`. These diagnostics
are part of the accepted-program restriction and are tested separately from
layout and macro evidence. Darwin's binary64 representation has native fixed
and variadic transport.

The enabled `x86_64-unknown-linux-gnu` SysV boundary profile does not yet
provide native x87 transport or an address-backed scalar `long double` value.
A call, definition, return, or variadic fetch using `long double` is therefore
rejected. The same profile rejects an aggregate containing `long double`, even
when the psABI would ultimately pass that aggregate in memory: accepting it
would also require 16-byte overflow and fixed-argument placement that the
native Cranelift `StructArgument` interface cannot describe. Declarations and
layout queries remain valid. The profile is explicitly versioned so an
address-only generated bridge capability can remove this accepted-program
restriction coherently.

## Variadic fetch restrictions

The requested type in `va_arg` must be complete, non-variably-modified,
object-sized, and unchanged by the default argument promotions. The target
`va_list` operand must designate modifiable list state.
CCC rejects promotion-invalid requests such as `float`, `_Bool`, character and
short integer types, and promotion-affected enumerations.

C specifies undefined behavior when an executed `va_arg` requests a type that
is incompatible with the promoted actual argument; it does not require a
translation diagnostic for an unreachable request. CCC deliberately applies a
hard semantic diagnostic even when the expression is unreachable. This is a
fail-loud accepted-program restriction: CCC never silently fetches a promoted
`double` or `int` using the wrong layout. Ordinary mismatches between otherwise
valid requested types remain runtime undefined behavior and are not dynamically
checked.

## `_Complex` and `_Imaginary`

When complex arithmetic is not enabled for a target configuration, CCC defines `__STDC_NO_COMPLEX__` to `1`, does not claim the corresponding GNU capability, and diagnoses semantic use of `_Complex`, `_Imaginary`, or complex builtins. The parser still recognizes the syntax so the diagnostic is precise. `<complex.h>` either comes from a compatible libc that observes `__STDC_NO_COMPLEX__` or from a CCC wrapper that reports the unsupported capability; it must not expose declarations that would later miscompile.

A conforming complex implementation represents values as typed real/imaginary pairs in CCC-IR, defines arithmetic and exceptional behavior, and supplies explicit per-target ABI plans. Merely accepting the keywords does not enable the capability.

## Variable-length arrays

C11 makes runtime-sized automatic VLA objects optional and does not require
their physical storage to reside on the machine stack. Variably modified types
remain representable independently of that storage capability. Semantic
analysis distinguishes an expression-bound array from prototype-scope `[*]`,
retains supported parameter and local-declaration extents, permits fixed-size
objects such as pointers to VLA where C permits them, and diagnoses illegal
storage classes. Variably modified typedef and type-name bounds remain explicit
frontend boundaries until their effects can be represented without loss.

The hosted profile implements automatic VLA object allocation through the
[runtime-sized automatic storage contract](cranelift-risks.md#runtime-sized-automatic-storage-contract),
including checked extents, multidimensional strides, alignment, bounded reuse,
and normal-return cleanup. It still defines `__STDC_NO_VLA__` to `1` until the
remaining runtime-layout and variably modified type contexts have complete
semantic, CCC-IR, provider, failure, and execution evidence. The macro describes
the complete optional C11 capability; it does not prevent a documented subset
from being accepted as an extension and does not promise native-stack storage.

The selected hosted provider is the scoped arena in
[ADR-0011](../adr/0011-arena-backed-runtime-sized-automatic-storage.md). It
does not enable `__builtin_alloca` or related native-stack builtins;
`__has_builtin` reports them as unavailable until the separate
[native dynamic-stack contract](cranelift-risks.md#native-dynamic-stack-capability-contract)
is proved. The arena provider documents possible storage loss across nonlocal
exit and does not claim POSIX async-signal-safe allocation. A GNU statement
expression does not extend a VLA's lifetime beyond its closing brace, even when
its result contains a pointer to that storage.

## Effective implementation-defined behavior

Implementation-defined answers are queried from one immutable effective configuration, not directly from a triple. It combines target defaults with language, ABI, code-generation, and toolchain options as specified in [Targets](targets.md#effective-compilation-configuration). It owns:

- plain `char` signedness, including `-fsigned-char` / `-funsigned-char`;
- integer widths and the underlying types of `size_t`, `ptrdiff_t`, `wchar_t`, `wint_t`, and `intmax_t`;
- enum selection, including `-fshort-enums`;
- bitfield and aggregate layout, packing pragmas, and packing/alignment flags;
- signed representation, right-shift behavior, and out-of-range conversions;
- `long double`, calling-convention, ISA, relocation, TLS, and code-model options;
- every predefined macro and builtin-header value derived from those choices.

Signed overflow is undefined by default. `-fwrapv` changes IR arithmetic to wrapping semantics; `-fno-strict-overflow` constrains optimizations without changing the abstract-machine result; either flag is rejected if its complete semantics are unavailable.

Differential testing compares implementation-defined results only under an identical effective configuration.

## GNU compatibility registry

GNU spellings and capabilities are described by a versioned registry shared by the preprocessor, parser, semantic analysis, diagnostics, and driver. Every attribute, builtin, pragma, extension, and compatibility macro is in exactly one state:

- **Implemented:** syntax and observable semantics are supported and tested.
- **Behavior-compatible no-op:** ignoring it cannot affect layout, ABI, control flow, memory semantics, linking, or program output; the registry documents why.
- **Parse-only:** retained in the AST for header parsing, but semantic use or code-generation reachability produces a hard diagnostic.
- **Unsupported:** rejected at the point of use.

The exact alternative-keyword, attribute, declaration, and phase-certification
rules are the [frontend capability contract](frontend-capabilities.md). Syntax
recognition alone does not advance a hosted-header profile beyond its recorded
phase.

The activation semantics and proof obligations for the selected C11 and GNU
constructs are defined separately in the
[C11 and GNU semantics contract](core-c11-and-gnu-semantics.md). In particular,
the advertised GCC version is only one component of CCC's effective identity;
CCC-provided predicates and later language facts are enumerated there as
intentional deviations rather than being attributed to historical GCC 4.2.1.

Layout, calling-convention, visibility, aliasing, section, TLS, cleanup, control-flow, vector, and code-generation attributes can never be classified as no-ops. Unknown attributes are preserved for diagnostics and rejected unless the standard explicitly permits them to be ignored and doing so is behavior-safe.

`__has_attribute`, `__has_builtin`, `__has_feature`, and related predicates
return true only for registry entries whose promised behavior is implemented
for the current effective configuration. Parse-only support returns false.
`__has_include` and `__has_include_next` report resolver results without opening
a second, inconsistent search path; direct header-name operands are preserved
while computed operands use normal macro expansion.

CCC always defines `__CCC__` and a CCC version tuple. `__GNUC__` and its version macros are defined only when a named GNU compatibility profile is active. Each profile has a checked manifest of the unguarded syntax and semantics that headers may infer from that GCC version; CCC does not raise the advertised version until the manifest passes on every target that exposes it.

The manifest distinguishes claims needed to select a hosted header's
preprocessing path from claims needed to parse or compile the resulting
declarations. A preprocessing-only invocation may use a checked
header-selection manifest. An invocation that continues into parsing requires
the parser entries inferred by the same GNU version and retains parse-only
constructs in the AST. Semantic analysis and object emission each require their
own stronger capability ceiling; a parsing profile never promotes parse-only
entries implicitly. The exact advertised GCC version is data in the manifest,
and changes to it require an audit of every version gate exercised by the
pinned libc-header corpus.

A GNU profile is not optional on hosted Linux targets. When `__GNUC__` is absent or ancient, glibc and musl headers take a fallback path that erases attributes and related keywords by macro (`sys/cdefs.h` defines `__attribute__(xyz)` to nothing), silently changing declarations, layout, and ABI — outside CCC's own no-silent-change machinery, because it happens by macro expansion inside libc. The apparently conservative option is the unsafe one. A hosted target's capability manifest therefore includes a minimum claimed GCC version, its header gates run with that profile active, and compiling against a hosted libc without an active GNU profile is refused rather than allowed to degrade silently.

Supported GNU syntax includes only registry entries with the state above. Inline assembly is represented with templates, operands, constraints, clobbers, volatility, and `asm goto` labels; code generation follows the bridge/whole-function rules in the [Cranelift risk register](cranelift-risks.md#risk-register). Assembly labels on declarations (`int f(void) __asm__("f_impl");`) are a separate registry capability from inline-assembly bodies: they change the linked symbol name, can never be no-ops, and glibc's `__REDIRECT` machinery makes them a requirement for hosted compiles.
