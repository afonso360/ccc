# Frontend capability contract

This document is the auditable acceptance inventory for the compiler's current
`x86_64-unknown-linux-gnu` configuration. It distinguishes syntax that reaches
the untyped AST from semantics that reach CCC-IR and object emission. The
broader architecture documents describe intended invariants; this inventory is
the authority for what an invocation may rely on today.

The target behavior and evidence required to advance the selected C11 and GNU
constructs are specified in
[Core C11 and selected GNU semantics](core-c11-and-gnu-semantics.md). That
document does not promote a current boundary by itself.

The implementation authorities are
[`ccc-syntax::frontend`](../../crates/ccc-syntax/src/frontend/mod.rs),
[`ccc-sema::generic`](../../crates/ccc-sema/src/generic/analyze.rs),
[`ccc-types`](../../crates/ccc-types/src/), and the
[`CapabilityRegistry`](../../crates/ccc-target/src/lib.rs). The corresponding
black-box evidence lives in the driver
[`execution`](../../crates/ccc-driver/tests/execution.rs),
[`diagnostics`](../../crates/ccc-driver/tests/diagnostics.rs),
[`ABI oracle`](../../crates/ccc-driver/tests/abi_oracle.rs), and
[`object-emission`](../../crates/ccc-driver/tests/object_emission.rs) tests.

## Capability states and compiler boundaries

Frontend capabilities use the states defined by the
[conformance policy](conformance.md#gnu-compatibility-registry): implemented,
behavior-compatible no-op, parse-only, and unsupported. Those states apply to
one named construct, not to a family inferred from similar syntax.

- **Preprocessing** covers header selection, macro expansion, feature
  predicates, includes, pragmas, and deterministic textual output.
- **Parsing** covers phase-7 token conversion, declaration grammar, scope
  events needed for typedef-name disambiguation, and the untyped AST.
- **Semantic analysis** covers name and type resolution, composite types,
  constant expressions, explicit conversions, storage duration, and layout.
- **Object emission** covers verified CCC-IR, ABI planning, sections, symbols,
  relocations, and machine code.

A later boundary requires all earlier boundaries. Parsing a construct does not
grant semantic support, and a semantically valid declaration is not proof that
its function ABI can be emitted. `--dump-tokens`, `--dump-ast`, and
`--dump-typed-ast` request their actual boundary rather than sharing a generic
"continues after preprocessing" permission.

Phase-7 conversion decodes integer, floating, character, and string constants
once, concatenates adjacent string literals, and preserves intervening pragma
and location events. The six C digraphs are canonicalized exactly: `<:` and
`:>` become brackets, `<%` and `%>` become braces, and `%:` and `%:%:` become
the preprocessing `#` and `##` punctuators. Unsupported punctuation is retained
as an unsupported token and receives the ordinary spanned parser diagnostic;
it is not guessed from a similar spelling.

## Declaration and declarator inventory

### Declaration forms and type specifiers

| Surface                   | Accepted forms                                                                                                                       | Current semantic and emission contract                                                                                                                                                                                                                                                                                              |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Translation-unit items    | Object and function declarations, function definitions, type-only declarations, `_Static_assert`, and ordered pragmas                | Implemented. External items retain source order while declarations use stable arena IDs.                                                                                                                                                                                                                                            |
| Block items               | Declarations, typedefs, function declarations, `_Static_assert`, statements, and ordered pragmas                                     | Implemented. File, function, prototype, block, and `for` scopes are distinct.                                                                                                                                                                                                                                                       |
| Storage classes           | `typedef`, `extern`, `static`, `auto`, `register`, `_Thread_local`                                                                   | Context rules are checked. Automatic, static, and thread storage durations are represented; block `extern` resolves to file scope. GNU `__thread` remains a separate parse-only registry spelling.                                                                                                                                  |
| Function specifiers       | `inline`, `_Noreturn` and reserved `__inline` spellings                                                                              | Parsed and retained as function properties. No optimization is inferred from `inline`; complete ISO inline-definition/linkage behavior is not advertised separately.                                                                                                                                                                |
| Type qualifiers           | `const`, `restrict`, `volatile`, `_Atomic`                                                                                           | Qualifiers remain on declarations and places; value conversion produces an unqualified rvalue type. `const` modification is rejected and volatile accesses are ordered. Atomic access reaches a hard backend boundary (`CCC4011`).                                                                                                  |
| Scalar types              | `void`, `_Bool`, plain/signed/unsigned `char`, signed/unsigned `short`, `int`, `long`, `long long`, `float`, `double`, `long double` | Integer width, rank, signedness, promotions, and target plain-`char` behavior are implemented. `float` and `double` scalar operations are implemented. `long double` size/alignment are implemented, while literals requiring conversion, arithmetic, initialization, calls, and returns are hard boundaries (`CCC2343`/`CCC3509`). |
| Tags                      | Named and anonymous `struct`, `union`, and `enum`; forward declarations; definitions; nested tag scopes                              | Implemented with nominal identity and deterministic translation-unit-local anonymous names. Redefinition and cross-category tag conflicts are diagnosed.                                                                                                                                                                            |
| Typedef names             | File- and block-scope typedef declarations, shadowing, use in declarations and type names                                            | Implemented. Parser-owned `NameClassEnv` records point-of-declaration events and rolls back tentative parses.                                                                                                                                                                                                                       |
| C11 alignment             | `_Alignas(type)`, `_Alignas(expression)`                                                                                             | Implemented for file, block-static, automatic, structure-member, and union-member objects. Requests reach record layout, data emission, automatic stack slots, ABI digests, and execution; invalid subjects, weakening requests, conflicting redeclarations, and backend-inexpressible extended alignments receive `CCC2437`.                                                                      |
| Complex type spellings    | `_Complex`, `_Imaginary`                                                                                                             | Syntax is retained and semantic analysis rejects these types with `CCC2216`.                                                                                                                                                                                                                                                        |
| Type-producing extensions | `typeof(type-name)` and `typeof(expression)`, including reserved spellings                                                           | Parse-only; semantic use is rejected with `CCC2214`.                                                                                                                                                                                                                                                                                |

`_Static_assert` requires an integer constant expression and fails with
`CCC2269` when false. `_Atomic(type-name)` and the `_Atomic` qualifier are
represented so an attempted access receives the atomic-capability diagnostic
instead of being compiled as an ordinary access.

The canonical built-in `TypeId` prefix is append-only. Adding an extension
integer type appends its hand-assigned constant, `BuiltinType` entry, and
`TypeStore` initialization entry in lockstep; existing built-in IDs are never
renumbered. Mapping tests cover all three coupled representations before a new
type can enter ABI or IR encoding.

### Derived declarators

| Declarator shape            | Accepted forms                                                                                                                                     | Current boundary                                                                                                                                                                                                                                                                                                                                                                                                           |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pointers                    | Arbitrary pointer nesting; `const`, `restrict`, `volatile`, and `_Atomic` pointer qualifiers; parenthesized binding                                | Implemented. Pointee qualification is distinct from top-level qualification.                                                                                                                                                                                                                                                                                                                                               |
| Arrays                      | Constant bounds, omitted bounds, `[*]`, runtime bounds, and parameter-array `static`/qualifiers                                                    | Prototype `[*]` and nonconstant bounds are distinct from incomplete arrays. Definition-parameter and supported local-declaration extents are retained once; earlier parameters are visible to later bounds. Fixed-size pointer-to-VLA objects are accepted. Runtime-sized object storage and layout remain unavailable.                                                                                                    |
| Functions                   | Fixed prototypes, `(void)`, an unspecified parameter list `()`, variadic syntax, nested function pointers, and array/function parameter adjustment | Fixed scalar and aggregate prototypes, prototyped variadic boundaries, and calls through an unspecified parameter list are implemented for `x86_64-unknown-linux-gnu`. Unprototyped calls use the promoted actual types and the SysV variadic register protocol. A residual unspecified-signature definition is an ABI hard boundary (`CCC3506`). A compatible later prototype forms the order-independent composite type. |
| Old-style definitions       | Identifier lists and following parameter declarations                                                                                              | Preserved in the syntax tree; semantic analysis rejects the identifier-list function type (`CCC2224`).                                                                                                                                                                                                                                                                                                                     |
| Abstract declarators        | Pointers, arrays, and functions in casts, `sizeof`, `_Alignof`, parameters, and type names                                                         | Constant-layout forms are implemented. A runtime bound in a type name is rejected with `CCC2417` until the bound effect can be retained without loss.                                                                                                                                                                                                                                                                      |
| Attributes                  | Before specifiers, within specifier sequences, after declarators/prototypes, and on tags, members, enumerators, and labels                         | Enumerator attributes are accepted before or after the enumerator name. Balanced arguments and original spelling are retained. Semantic status is determined only by the exact registry entry below.                                                                                                                                                                                                                         |
| Declaration assembly labels | Reserved or GNU-mode `asm` after a declarator, before later attributes or `;`                                                                      | Implemented for declarations that denote function or object symbols. C lookup keeps the source identifier while typed declarations, IR, relocations, and definitions use the decoded assembly symbol. Redeclarations must agree (`CCC2419`); automatic and block-static objects are rejected (`CCC2257`).                                                                                                                  |

`struct` and `union` members include named fields, unnamed record members,
ordinary fields, named and unnamed bit-fields, zero-width bit-field barriers,
and a valid final flexible array member. The flexible member contributes its
element alignment and a zero-sized tail layout; invalid union, non-final, or
member-only forms receive `CCC2370`, and initialization receives `CCC2431`.

Enumeration values are integer constant expressions. The complete value range
selects a representable target integer type, including an unsigned type for
large nonnegative values; later references and promotions use that selected
underlying type.

## Expression and conversion inventory

### Expressions

| Expression family          | Implemented forms                                                                                                    | Explicit boundary                                                                                                                                                                                                                                                                                             |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Primary and literals       | Identifiers, including C11 `__func__`; integer, floating, character, and ordinary/`u8`/`u`/`U`/`L` string literals; parenthesized expressions | Integer literal candidates follow radix and suffix rules. Ordinary character constants apply target plain-`char` signedness. Adjacent strings are concatenated before parsing. Native `long double` operations remain unavailable.                                                                            |
| Postfix                    | Subscript, direct and indirect calls, `.` and `->`, postfix `++`/`--`, and block-scope compound literals            | Compound literals are addressable lvalues with enclosing-block automatic storage; each evaluation runs the initializer against the occurrence's retained object. File-scope occurrences are rejected with `CCC2430`.                                                                                          |
| Unary                      | Prefix `++`/`--`, address, dereference, unary `+`/`-`, bitwise `~`, logical `!`                                      | Implemented with lvalue/modifiability checks, target signedness, and explicit loads/stores.                                                                                                                                                                                                                   |
| Layout operators           | `sizeof expression`, `sizeof(type-name)`, `_Alignof(type-name)`, `__builtin_offsetof` with nested member/index paths | Implemented for types with a compile-time layout. `offsetof` rejects bit-fields and nonconstant indices. Runtime VLA layout is not implemented.                                                                                                                                                               |
| Scalar constant builtins   | `__builtin_huge_val()`, `__builtin_inff()`, and `__builtin_nanf("")`                                                 | Fold to positive binary64 infinity, positive binary32 infinity, and the canonical binary32 quiet NaN respectively. The NaN form also accepts an empty `u8` literal; nonliteral, wide, and nonempty payloads are rejected rather than approximating payload semantics.                                         |
| Integer builtins           | `__builtin_bswap64`, `__builtin_clz`, `__builtin_clzl`, `__builtin_clzll`, `__builtin_ctzll`, `__builtin_popcount`, and `__builtin_popcountll` | Use their exact GNU argument and result types and lower to native Cranelift `bswap`, `clz`, `ctz`, and `popcnt` operations. Count-leading/trailing-zero with a zero operand retains GNU's undefined-behavior contract. Other family spellings, including `__builtin_bswap32`, are not inferred. |
| Prefetch builtin           | `__builtin_prefetch(address[, rw[, locality]])`                                                                       | Behavior-compatible no-op: converts the address to `const void *` and evaluates it exactly once, accepts only integer constant hints in the GNU ranges, and emits no memory access. The default hints are read and locality 3.                                                                            |
| Legacy atomic builtins     | `__sync_add_and_fetch`, `__sync_fetch_and_add`, `__sync_sub_and_fetch`, `__sync_bool_compare_and_swap`, `__sync_val_compare_and_swap`, `__sync_lock_test_and_set`, and `__sync_synchronize` | The operation builtins accept modifiable 1, 2, 4, or 8-byte integer and pointer objects and lower to native sequentially consistent atomic RMW/CAS instructions. This intentionally strengthens the acquire-only minimum ordering of GNU lock/test-and-set. The optional protected-variable list is analyzed but not evaluated. |
| Casts                      | Arithmetic conversions, pointer/integer casts, pointer/pointer casts, casts to `_Bool` and `void`                    | Implemented for scalar types other than native `long double`.                                                                                                                                                                                                                                                 |
| Arithmetic and bitwise     | `*`, `/`, `%`, `+`, `-`, shifts, `&`, `^`, and bitwise-or                                                            | Integer promotions and usual arithmetic conversions are explicit. Pointer addition/subtraction is scaled; pointer difference uses the target `ptrdiff_t` type.                                                                                                                                                |
| Comparison and logic       | `<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, and logical-or                                                               | Arithmetic and compatible-pointer comparisons are implemented; both logical operators short-circuit in the CFG.                                                                                                                                                                                               |
| Conditional                | `?:`                                                                                                                 | Implemented for compatible arithmetic, pointer, `void`, and aggregate operands; aggregate branches produce independent owned snapshots.                                                                                                                                                                       |
| Variadic builtins          | `__builtin_va_start`, `__builtin_va_arg`, `__builtin_va_copy`, and `__builtin_va_end`                                | Implemented for the target-derived `__builtin_va_list` type. Context, final named parameter, modifiability, complete requested types, default promotions, and the enabled boundary profile are checked semantically.                                                                                          |
| Assignment                 | `=`, all arithmetic/shift/bitwise compound assignments, prefix/postfix increments                                    | Implemented with one evaluation of the target place. Aggregate assignment uses an overlap-safe copy; volatile aggregate reads and writes expand into ordered accesses.                                                                                                                                        |
| Comma and discarded values | Comma expressions and expression statements, including lvalues whose result is unused                                | Implemented. Discarding a value does not suppress required volatile scalar, member, array, bit-field, or aggregate reads.                                                                                                                                                                                     |
| Generic selection          | `_Generic` syntax and associations                                                                                   | Parsed, then rejected with `CCC2270`.                                                                                                                                                                                                                                                                         |
| GNU expression marker      | `__extension__ expression`                                                                                           | Behavior-compatible no-op; it only suppresses diagnostics CCC does not emit.                                                                                                                                                                                                                                  |
| GNU label values           | `&&label` in `gnu11`                                                                                                 | Produces an opaque pointer-typed token for a label in the current function. Direct pointer tables, copying, storage, equality, and computed jumps are implemented. Arithmetic receives `CCC2425` while direct label provenance remains detectable; post-storage arithmetic is outside the supported behavior. |

Each function definition provides one `__func__` object with the lexical
function name, as if declared `static const char __func__[]` at the start of
the body. Repeated uses share its static-storage address, while unrelated
string literals retain separate object identity. The object is materialized
only when used. GNU function-name aliases are not inferred from this standard
identifier.

GNU statement expressions and inline assembly expressions/bodies are not part
of the current grammar. Declaration assembly labels are the separate
symbol-renaming construct listed above.

### Explicit typed-AST conversions

The typed AST has a closed conversion inventory in
[`ConversionKind`](../../crates/ccc-sema/src/generic/model.rs):

| Conversion                                | Inserted for                                                                                                           |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `LvalueToValue`                           | Every object read, carrying volatile/atomic access metadata                                                            |
| `ArrayToPointer`, `FunctionToPointer`     | The standard value conversions outside their excluded contexts                                                         |
| `IntegerPromotion`                        | Narrow integers, `_Bool`, `char`, and enumerations                                                                     |
| `IntegerConversion`, `FloatingConversion` | Same-category arithmetic conversions                                                                                   |
| `IntegerToFloating`, `FloatingToInteger`  | Mixed arithmetic assignment, casts, and usual arithmetic conversions                                                   |
| `PointerConversion`                       | Null pointer constants, compatible object pointers, `void *`, and explicit integer/pointer casts                       |
| `QualificationAdjustment`                 | Reserved as a distinct IR-supported category; current compatible pointer qualification changes use `PointerConversion` |
| `ToBoolean`                               | Conditions, logical operators, and `_Bool` assignment/casts                                                            |
| `ToVoid`                                  | Explicit casts to `void`                                                                                               |

Default argument promotions are represented (`float` to `double` and integer
promotions) and consumed by prototyped variadic and unprototyped call plans. On
SysV AMD64, an unprototyped call is placed from that complete promoted actual
type list and sets `%al` to the number of used SSE argument registers.
Relocation-bearing constants remain symbol-plus-addend values through conversion
instead of being flattened to integers. SSA value types are
top-level-unqualified `TypeId`s; qualifiers and access semantics remain on
declarations, places, loads, and stores.

## Layout and initialization inventory

All layout queries go through
[`TypeStore::layout_of`](../../crates/ccc-types/src/layout.rs) using the
effective target data layout and record-local packing policy. Results are
cached by type, target layout, and packing configuration; type completion
invalidates affected entries.

| Area                 | Current contract                                                                                                                                                                                                                       |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Scalars and pointers | Sizes and alignments derive from `TargetDataLayout`; the enabled target is little-endian LP64 with signed plain `char`, 32-bit `int`, 64-bit `long`/pointers, and a 16-byte `long double` object.                                      |
| Arrays               | Layout records length and element stride. Incomplete arrays may appear where C permits and become complete through a compatible redeclaration or initializer. Variable length remains an explicit non-layout result.                   |
| Structures           | Source-order fields, natural padding/alignment, anonymous record members, nested records, packed records, and bit-field allocation are target-derived.                                                                                 |
| Unions               | Size/alignment are the maxima of members; ordinary and bit-field union members share offset zero according to target policy.                                                                                                           |
| Bit-fields           | Declared type, signedness, storage offset/size/alignment, bit offset, width, zero-width barriers, packing, and mixed declared types are explicit. The x86-64 oracle compares zero baselines and XOR deltas against both GCC and Clang. |
| Packing pragmas      | `pack()`, `pack(0)`, `pack(n)`, `push`, `pop`, named frames, and combined label/alignment forms are implemented for `n = 1, 2, 4, 8, 16`; zero restores native alignment. Malformed, unmatched, or unknown forms are hard diagnostics. |
| Layout operators     | `sizeof`, `_Alignof`, and `__builtin_offsetof` consume the same layout result used by initialization, ABI planning, and code generation.                                                                                               |

Initializers accept scalar expressions, brace-enclosed aggregate lists,
designated array/member paths, nested aggregates, string-to-character-array
initialization, union member selection, bit-fields, and implicit zero fill.
Incomplete arrays are completed from list or string length. Excess elements,
invalid or out-of-range designators, incompatible assignments, and nonconstant
static initializers are diagnosed before object emission.

Static data is retained as a verified initializer graph containing zero,
target-endian scalar data, symbol-plus-addend relocations, encoded string data,
aggregate edges, and repeated fragments. Repetition denotes identical adjacent
subobjects without changing their required stride or alignment. Object emission
resolves the graph into `.data`, `.bss`, and `.rodata` bytes plus relocations;
bit-field fragments update only their allocated bits.

String pooling is keyed by element type, encoding, complete code-unit sequence
including the terminator, required alignment, and mutability mode. It does not
merge ordinary, UTF-8, wide, UTF-16, or UTF-32 objects merely because their raw
bytes happen to match.

## Linkage, storage, and control-flow inventory

### Names, storage, and object files

| Area                   | Current contract                                                                                                                                                                                                                                                                                                                                      |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Linkage                | File objects and functions have internal or external linkage; block declarations without linkage remain distinct. Incompatible redeclarations are errors. `extern` followed by `static` is rejected at the later declaration (`CCC2372`) for objects and functions.                                                                                   |
| Composite declarations | Compatible incomplete/complete arrays and unspecified/prototype function declarations form a composite type independent of declaration order. Top-level parameter qualifiers are ignored when forming function types.                                                                                                                                 |
| Definitions            | Multiple initialized object definitions and multiple function definitions are rejected. Tentative external objects become ELF common symbols with target size/alignment; an initialized definition supersedes a tentative declaration.                                                                                                                |
| Static locals          | Block statics use translation-unit-local ELF data symbols and require constant initialization. They are never initialized by an automatic-entry store.                                                                                                                                                                                                |
| Automatic locals       | Non-address-taken, nonvolatile scalar locals are promoted to SSA. Address-taken, volatile, aggregate, atomic, and variably modified locals retain explicit storage.                                                                                                                                                                                   |
| Strings and globals    | Objects and functions carry symbol name, binding, and visibility through semantic analysis, IR, ABI planning, and ELF emission. `visibility("default")`, `visibility("hidden")`, `visibility("protected")`, and `visibility("internal")` are implemented; other layout and linkage override attributes remain outside the default supported registry. |
| ELF proof              | Object tests inspect `.text`, `.data`, `.bss`, `.rodata`, local/global/undefined bindings, data and string relocations, and `R_X86_64_PLT32` direct external calls. Linux tests cross-link CCC callers and callees with a reference compiler in both directions and prove same-spelled `static` names stay local.                                     |

Standard `_Thread_local` objects are represented through TLS storage and
relocation nodes, but the current acceptance suite does not certify a
cross-linked TLS access model. It is therefore not part of the advertised
cross-link contract. The GNU `__thread` spelling remains parse-only in the
registry even though it is recognized by token conversion.

### Statements and CFG behavior

| Statement               | Current contract                                                                                                                                                                                                                                                                                      |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Compound and expression | Implemented with lexical block scopes; empty and discarded-value expressions are valid.                                                                                                                                                                                                               |
| Selection               | `if`/`else` and `switch` with integer-promoted controlling expressions are implemented. Duplicate `case`, duplicate `default`, and nonconstant cases are diagnosed.                                                                                                                                   |
| Iteration               | `while`, `do`/`while`, and `for` with expression or declaration initializers are implemented. `for` declarations have their own scope.                                                                                                                                                                |
| Labels and jumps        | Ordinary labels, direct and computed `goto`, `break`, `continue`, and `return` are implemented. Duplicate/undefined labels, nonpointer computed targets, label arithmetic, and jumps outside the required loop/switch context are diagnosed.                                                          |
| Expression CFG          | Short-circuit logic and `?:` use explicit blocks and block parameters. Loop/switch exits and direct `goto` are verified CFG edges; computed `goto` uses a dense per-function `br_table` with a trapping default.                                                                                      |
| Calls                   | Direct and function-pointer calls use the same module ABI plan. Supported scalar and aggregate fixed boundaries lower natively; `x86_64-unknown-linux-gnu` prototyped variadic and unprototyped calls use generated bridges. Unsupported native-`long double` boundaries fail before object emission. |

## Exact GNU compatibility registry

The following tables transcribe
[`CapabilityRegistry::gnu_frontend`](../../crates/ccc-target/src/lib.rs). The
key is `(kind, name)`; similarly spelled keys do not inherit one another's
state. Aggregate manifest keys such as `gnu-alternative-keywords` describe the
weakest member of that advertised surface, so individual reserved qualifier
spellings can be implemented while the aggregate key remains parse-only.

### Extensions

| State                     | Exact keys                                                                                                                                                                                                                  |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Implemented               | `__const`, `__const__`, `__inline`, `__inline__`, `__restrict`, `__restrict__`, `__signed`, `__signed__`, `__volatile`, `__volatile__`, `__alignof`, `__alignof__`, `gnu-declaration-asm-labels`, `gnu-restrict-qualifiers` |
| Behavior-compatible no-op | `__extension__`, `gnu-extension-marker`                                                                                                                                                                                     |
| Parse-only                | `__asm`, `__asm__`, `__attribute`, `__attribute__`, `__typeof`, `__typeof__`, `__thread`, `gnu-alternative-keywords`, `gnu-attribute-specifiers`, `gnu-typeof`                                                              |
| Unsupported               | Every other extension key, including plain unreserved spellings in strict `c11` mode                                                                                                                                        |

Plain `asm` and `typeof` are accepted only in `gnu11`. Reserved alternatives
remain recognizable in strict `c11` because hosted headers use them to avoid
changing the caller's language mode. A spelling that token conversion maps to
a standard AST node does not acquire a stronger contract than its registry key.

### Attributes

| State                     | Exact keys and rationale                                                                                                                                                                                                                   |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Implemented               | `noreturn`, `__noreturn__`, record-specifier `packed` and `__packed__`, `mode`, `__mode__`, `aligned`, `__aligned__`, `weak`, `__weak__`, `visibility`, and the all-pointer inline-anonymous-typedef subset of `transparent_union` and `__transparent_union__`; their control-flow, type, layout, binding, visibility, or calling effects are retained through the applicable compiler stages. Other `packed` placements receive `CCC2432`; unsupported transparent-union forms receive `CCC2439`. |
| Behavior-compatible no-op | `nothrow`, `__nothrow__`, `pure`, `__pure__`, `const`, `__const__`, `malloc`, `__malloc__`, `format`, `__format__`, `nonnull`, `__nonnull__`, `warn_unused_result`, `__warn_unused_result__`, `unused`, `__unused__`, `deprecated`, `__deprecated__`, `noinline`, `__noinline__`, `always_inline`, `__always_inline__`, `may_alias`, `__may_alias__`, `alloc_size`, `__alloc_size__`; CCC emits no TBAA metadata or allocation-size optimization, so omitting those optimizer contracts does not change generated behavior. The `aligned(1), may_alias` scalar-typedef idiom is accepted narrowly because scalar memory operations are unaligned-safe; it does not advertise general alignment-bearing scalar typedefs. |
| Parse-only                | `gnu_inline`, `__gnu_inline__`; these have an observable semantic or emission effect that the default configuration does not advertise                                                                                                                                                                                                                             |
| Unsupported               | Every other attribute name, including empty or unknown names                                                                                                                                                                               |

Semantic analysis rejects parse-only and unknown attributes with `CCC2345`;
retaining balanced argument tokens in the AST is not permission to ignore
them.

### Builtins, features, and pragmas

| Kind    | Exact default entries                                                                                                                                                                                                                                                 |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Builtin | Implemented: `__builtin_offsetof`, `__builtin_expect`, `__builtin_huge_val`, `__builtin_inff`, `__builtin_nanf`, `__builtin_bswap64`, `__builtin_clz`, `__builtin_clzl`, `__builtin_clzll`, `__builtin_ctzll`, `__builtin_popcount`, `__builtin_popcountll`, `__builtin_va_start`, `__builtin_va_arg`, `__builtin_va_copy`, `__builtin_va_end`, `__sync_add_and_fetch`, `__sync_fetch_and_add`, `__sync_sub_and_fetch`, `__sync_bool_compare_and_swap`, `__sync_val_compare_and_swap`, `__sync_lock_test_and_set`, and `__sync_synchronize`. Behavior-compatible no-op: `__builtin_prefetch`. Every other builtin key, including `__builtin_bswap32`, is unsupported. |
| Feature | No default entries. Feature predicates are false unless the effective configuration explicitly inserts an implemented or behavior-compatible entry.                                                                                                                   |
| Pragma  | No generic registry entries. Ordered built-in handling implements `#pragma pack`, `#pragma once`, `#pragma GCC system_header`, and the supported `#pragma GCC diagnostic` forms; unknown semantic pragmas are rejected with `CCC2355`.                                |

`__has_attribute`, `__has_builtin`, `__has_feature`, and related predicates
report true only for registry states whose promised behavior is available.
Parse-only recognition returns false.

`__builtin_expect(value, expectation)` follows the advertised GNU 4.2
contract. Both operands are converted to `long`; after conversion, the
expectation must fold to a compile-time constant and has no runtime evaluation.
Statically unselected `?:`, `&&`, and `||` operands do not prevent folding.
Lowering emits only the value operand and does not require backend
branch-probability metadata.

`__builtin_huge_val()` and `__builtin_inff()` are zero-argument constant
expressions for positive binary64 and binary32 infinity. The certified NaN
surface is exactly `__builtin_nanf("")` and its empty UTF-8-literal equivalent;
both produce binary32 quiet NaN bits `0x7fc00000`. Runtime pointers, wide
literals, and nonempty payload strings receive `CCC2429`, because CCC does not
claim GNU NaN payload encoding. These builtins lower as constants and never as
host-libm calls.

The integer-intrinsic signatures are exact: `__builtin_bswap64` accepts and
returns target `uint64_t`, which is `unsigned long` in the x86-64 GNU profile;
`__builtin_clz` and `__builtin_popcount` accept
`unsigned int`; `__builtin_clzl` accepts `unsigned long`; and the `clzll`,
`ctzll`, and `popcountll` forms accept `unsigned long long`. Every count form
returns `int`. The ordinary assignment conversions happen before the native
Cranelift operation. CCC deliberately does not define the `clz` or `ctz`
result for zero, matching the GNU contract, and does not infer neighboring
spellings such as `__builtin_bswap32`.
Valid integer constant-expression operands fold for all seven forms and remain
usable in enumerators, static assertions, and array bounds. Zero-input `clz`
and `ctz` remain outside that fold at their undefined-behavior boundary.

`__builtin_prefetch` has the exact first-argument type `const void *`, including
ordinary null-pointer-constant conversion. Its optional hints are converted to
`int`; read/write must then be a constant from 0 through 1 and locality from 0
through 3. Their defaults are 0 and 3. Hints have no runtime evaluation. CCC
evaluates the converted address exactly once, then emits no load, store, or
other potentially faulting access. This preserves the observable behavior
required by the selected source while making no backend cache-hint promise.

The implemented legacy `__sync_*` operations carry an explicit sequentially
consistent atomic effect in CCC-IR. Fetch/add and lock/test-and-set return the
old object representation; add/fetch and sub/fetch derive the new value from
that atomic result; compare-and-swap returns either the observed value or an
`_Bool` success flag. Pointer-valued objects use their unscaled integer
representation. A nonpointer first argument, a const pointee, or an unsupported
object type receives `CCC2433` or `CCC2434`. Cranelift's native `atomic_rmw` and
`atomic_cas` operations provide the implementation; no external `__sync_*`
symbol, libc call, or non-atomic fallback is emitted. All supported forms use
sequentially consistent operations; for `__sync_lock_test_and_set`, this is an
intentional strengthening of GNU's acquire-only minimum guarantee.
Integer and pointer value operands convert in either direction through their
raw representation, including a `void *` null value for a function-pointer
object. CCC-IR records whether an RMW returns the old or derived new
representation, so the backend implements add/fetch and sub/fetch without
introducing pointer-typed binary arithmetic.

## Explicit unsupported boundaries

The current frontend never silently approximates these constructs:

| Construct                                                       | Furthest retained boundary                | Diagnostic or behavior                                                                                                                                                                                                                                                                        |
| --------------------------------------------------------------- | ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Variably modified types, object storage, and runtime VLA layout | Typed declaration/type representation     | Prototype `[*]` is distinct; supported parameter and local-declaration bounds are retained. Runtime-sized object storage is `CCC2258`; block `extern`, typedef, type-name, and block-function-declarator boundaries are `CCC2415`–`CCC2418`; retained effects fail explicitly at IR lowering. |
| General C11 `_Atomic` object access and operations              | Typed AST/CCC-IR boundary                 | `CCC4011`; the exact legacy `__sync_*` scalar builtin surface above is independent and does not enable ordinary `_Atomic` loads, stores, or compound updates                                                                                                                                  |
| Native `long double` arithmetic, initialization, calls, returns | Layout/type representation                | `CCC2343` or `CCC3509`; no implicit `double` substitution                                                                                                                                                                                                                                     |
| `_Complex`, `_Imaginary`                                        | Declaration syntax                        | `CCC2216`                                                                                                                                                                                                                                                                                     |
| `_Generic`                                                      | Expression syntax                         | `CCC2270`                                                                                                                                                                                                                                                                                     |
| File-scope compound literals                                    | Typed initializer and object-place model  | `CCC2430`; block-scope occurrences have automatic storage and are fully lowered                                                                                                                                                                                                               |
| Old-style identifier-list function definitions                  | Declarator syntax                         | `CCC2224`                                                                                                                                                                                                                                                                                     |
| Residual unspecified-prototype definition ABI                   | Typed signature                           | `CCC3506`; unprototyped calls use their promoted actual types through the generated SysV bridge                                                                                                                                                                                               |
| GNU `typeof`, unsupported or parse-only attributes              | Untyped AST with original spelling/tokens | `CCC2214` or `CCC2345` under the default registry                                                                                                                                                                                                                                             |
| Assembly labels on declarations without a linkable symbol       | Untyped AST with decoded label            | `CCC2257`; automatic and block-static object declarations are never silently renamed                                                                                                                                                                                                          |
| GNU statement expressions and inline assembly bodies            | Preprocessing tokens                      | Not in the current parser grammar                                                                                                                                                                                                                                                             |
| Unsupported targets, output modes, or command-line options      | Driver option/configuration               | Rejected before source compilation; no host fallback                                                                                                                                                                                                                                          |

An unused declaration with an unsupported call boundary may remain in an
object when no definition or call requires its ABI. The boundary is checked
when code generation must materialize that signature.

## Hosted-header code-generation evidence

The shipped
[`resource-dir/manifest.toml`](../../resource-dir/manifest.toml) advertises the
named GCC 4.2.1 compatibility profile through code generation. Its exact
shipped header inventory is four compiler-owned spelling
headers (`stdalign.h`, `stdbool.h`, `stdarg.h`, `stdnoreturn.h`) plus
target-derived `stddef.h`; it ships no hosted wrapper headers in this profile.

Builtin and inline-assembly inventories are preprocessed under CCC's effective
predefined-macro identity, including that exact advertised GNU version and the
registry-derived feature predicates. A host GCC or Clang identity is not
inventory evidence. Raising the advertised version is consequential because it
can select additional intrinsic and atomic paths, so the resulting semantics,
backend operations, and helper requirements must all be available and proved
before the profile changes.

The profile has five independent gates in
[`preprocessing.rs`](../../crates/ccc-driver/tests/preprocessing.rs) and the
dedicated [`header_parsing.rs`](../../crates/ccc-driver/tests/header_parsing.rs):

1. deterministic preprocessing of the pinned glibc-like fixture;
2. an AST dump of that fixture containing the required declarations,
   attributes, assembly label, restrict qualifiers, and inline definition;
3. Linux-only preprocessing of installed glibc `features.h`, `stddef.h`, and
   `stdint.h`, with exact type/macro sentinels; and
4. a Linux-only AST dump of installed glibc headers plus exact sentinel
   declarations for `typeof`, restrict, an assembly label, an attribute, and an
   inline definition; and
5. a Linux-only compile, link, and execution sentinel over installed glibc
   declarations, including the selected pthread and string-header attribute
   surface.

Installed-header gates log compiler identity, reported target, and libc
identity. They assert stable sentinel declarations rather than snapshotting a
mutable system header. The installed parsing gate proves only the advertised
syntax surface: parse-only GNU constructs remain in that AST fixture but are
not used by the code-generation sentinel. The execution gate separately proves
that the supported declaration subset reaches a native linked program.

Changing the advertised GNU version, the manifest ceiling, or a required
declaration spelling requires all of the following:

1. manifest validation rejects missing, duplicated, or unknown capability
   entries;
2. the curated fixture exercises each newly claimed spelling in context;
3. strict-`c11` and `gnu11` tests verify the intended alternative-keyword
   policy;
4. parse-only constructs remain visible in the AST; and
5. every action through the certified ceiling either preserves its required
   semantics or fails before emitting output.
