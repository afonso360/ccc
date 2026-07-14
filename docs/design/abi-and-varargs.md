# ABI lowering and variadic functions

This document defines how C calls cross the CCC-IR/Cranelift boundary. CCC-IR
retains source-level value semantics. Target-specific classification,
transport, generated bridge state, and packaging requirements are immutable
data produced by `ccc-abi` and consumed by code generation and the driver.

## Module ABI plan

`ccc-abi` constructs one `ModuleAbiPlan` after CCC-IR verification. Definition
plans are keyed by stable function IDs and call plans by stable function and
instruction IDs. A call plan includes the complete promoted actual argument
list, fixed variadic boundary, direct or indirect target identity, source
location, result storage, and required bridge operations.

The plan records an `AbiConfigKey` containing the normalized target triple,
data layout, calling convention, boundary-profile revision, classifier
revision, psABI source identity, and pinned backend profile. Absolute tool
paths, executable versions, sysroots, and packaging fingerprints are
operational provenance and are not part of ABI-plan identity or deterministic
`--dump-abi` output.

Planning stores a canonical digest of the verified IR. Plan verification is a
mandatory compiler operation immediately after planning and immediately before
dumping or code generation. The driver does not mutate IR after planning.

## Classification and placement authority

The SysV AMD64 classifier models `NO_CLASS`, `INTEGER`, `SSE`, `SSEUP`, `X87`,
`X87UP`, `COMPLEX_X87`, and `MEMORY`, including recursive merging, cleanup,
unaligned fallback, whole-aggregate register rollback, hidden-return
consumption, and stack-area calculation before placement.

Classification and transport are distinct:

- `ccc-abi` is authoritative for source classification, aggregate rollback,
  memory versus register transport, carrier order, copy sizes, and hidden
  return selection.
- Cranelift is the sole physical register and stack-placement authority for a
  native fixed boundary. A native plan records ordered `AbiParam` carriers and
  purposes but does not claim physical locations.
- A generated bridge plan owns exact registers, stack offsets, frame offsets,
  and marshal operations.

`--dump-abi` labels native placement as Cranelift-owned and prints the lowered
signature. It prints physical locations only for generated bridge plans.

The native backend profile is enabled only while a permanent cross-linked
compatibility test proves scalar interleaving, register rollback,
`StructArgument` placement and copy size, multi-value returns, and
`StructReturn` pointer echo behavior for the exact pinned Cranelift version and
settings.

## Supported boundary profile

The enabled `x86_64-unknown-linux-gnu` SysV boundary profile supports the
compiler's existing integer types, enumerations, pointers, `float`, `double`,
and aggregates recursively composed from those types with required alignment
no greater than eight.

Native `long double`, vector types, `_BitInt`, `__int128`, over-aligned types,
and aggregates containing them remain representable for declarations and
layout queries but are rejected when a definition, call, return, or `va_arg`
requires an unsupported boundary. The profile is versioned so an additional
profile can add address-only or generated-bridge transport without changing
the meaning of already produced plans.

## Aggregate values and fixed calls

Every aggregate rvalue in CCC-IR is an immutable owned snapshot. Parameter
materialization, assignment, conditional expressions, calls, and returns all
use that representation. Compiler-owned snapshot storage has an explicit
lifetime, and verifier-controlled field/index projection may derive an address
into it for array decay or subobject access. Generic storage-address operations
cannot expose compiler temporaries.

Volatile aggregate evaluation performs exactly one ordered observation.
Padding is never used as a C value. A memory-class argument is copied into a
zeroed ABI staging object whose allocation is rounded to the backend's
`StructArgument` size; only the logical source bytes are read. Register pieces
carry valid-byte ranges so loads and stores do not cross object bounds.

A hidden result uses fresh nonaliasing storage supplied by the caller. On SysV
AMD64 the hidden pointer consumes the first GP argument position and the callee
returns that exact pointer in `%rax`.

## Generated call bridge

Every bridged call uses a versioned `BridgeFrameV1` passed to a nonvariadic
assembly helper. The 16-byte-aligned fixed area is 256 bytes:

| Offset | Field                                                               |
| -----: | ------------------------------------------------------------------- |
|      0 | magic (`u32`), version (`u16`), header size (`u16`)                 |
|      8 | target address (`u64`)                                              |
|     16 | outgoing stack size and total frame size (`u32`, `u32`)             |
|     24 | advisory GP/XMM counts, `%al`, result counts, flags, reserved bytes |
|     32 | six eight-byte GP argument slots                                    |
|     80 | eight sixteen-byte XMM argument slots                               |
|    208 | `%rax` and `%rdx` result slots                                      |
|    224 | `%xmm0` and `%xmm1` result slots                                    |
|    256 | outgoing stack payload                                              |

The producer zeroes the complete fixed area. The helper holds the target in
`%r11`, preserves the frame pointer across the target call, copies the stack
payload with the required alignment, unconditionally loads all GP and XMM
slots, and writes `%al` last. It unconditionally captures the supported result
registers. Header counts are diagnostic metadata, not dispatch inputs.

The helper does not use the red zone and does not modify the x87 control word,
MXCSR control bits, or direction flag. It carries explicit CFI and a
non-executable-stack note. Bridge assembly intentionally has no `.file` or
`.loc` directives because assembler-produced line tables can encode the build
working directory. Debuggers can unwind through generated bridges but do not
provide source-level stepping within them. Same-compilation magic and version
validation happens in Rust before assembly materialization; production bridge
code does not contain an unreachable protocol trap.

## Variadic definitions and `va_list`

A variadic definition has an externally visible ABI-valid assembly entry and a
generated private nonvariadic CLIF body. Same-translation-unit calls and
function addresses use the public entry.

The entry snapshots incoming `%al` before using `%rax`, saves the required GP
and XMM registers, computes the initial overflow address, and constructs a
208-byte `VaStateV1`:

| Offset | Field                                   |
| -----: | --------------------------------------- |
|      0 | magic (`u32`), version and size (`u16`) |
|      8 | `gp_offset` (`u32`)                     |
|     12 | `fp_offset` (`u32`)                     |
|     16 | `overflow_arg_area` pointer             |
|     24 | `reg_save_area` pointer                 |
|     32 | 176-byte SysV AMD64 register-save area  |

The public `va_list` view starts at byte eight and is the psABI array-of-one
24-byte structure. Its register-save area has six eight-byte GP slots followed
by eight sixteen-byte XMM slots.

`va_arg` is one effectful ABI-neutral CCC-IR operation with an immutable
`VaArgPlan`. Code generation expands it into register, overflow, and merge
blocks. A `MEMORY` argument goes directly to the overflow path. For register
arguments every required GP and SSE slot is checked before either cursor is
updated; if either class is exhausted the entire argument comes from the
overflow area. Mixed values are reconstructed in owned storage.

Promotion-invalid requests such as `va_arg(ap, float)` are rejected with a
stable semantic diagnostic. Ordinary dynamic type mismatch remains C undefined
behavior and is not checked at runtime.

## Header contract

`stdarg.h` is compiler-owned, target-invariant interface text. It defines
`va_list` and guarded `__gnuc_va_list` aliases of the reserved
`__builtin_va_list` type and maps the standard operations to compiler builtins.
The target owns the canonical builtin type and layout; the header does not
spell target fields or offsets.

## Object packaging

Code generation returns a primary object, deterministic bridge assembly,
logical bridge locations for diagnostic inventory, required capabilities, and
an exact private-symbol allowlist. Logical bridge locations are not serialized
as assembler line records. If assembly is present, the driver:

1. assembles it through the resolved target compiler driver;
2. combines it with the primary object through a driver-mediated relocatable
   link;
3. localizes only the exact generated-symbol allowlist with a compatible
   object copier;
4. verifies architecture, relocatable format, bindings, visibility,
   relocations, CFI, and non-generated symbol preservation;
5. publishes the final single object atomically.

Bridge-free object emission resolves none of these operational tools. Blanket
hidden-symbol localization is forbidden because user-defined hidden symbols
can still require cross-object resolution.

## Required validation

- canonical classifier cases and exact plan/digest snapshots;
- semantic C-to-IR aggregate snapshot goldens that do not freeze copy counts;
- fixed aggregate cross-linking with GCC and Clang in both directions;
- direct and indirect variadic calls with zero, one, eight, and more than eight
  floating actuals, including a disassembly assertion for `%al` saturation;
- GCC/Clang-created `va_list` values consumed by CCC and CCC-created lists
  consumed by `vsnprintf` and `vfprintf`;
- register and overflow exhaustion, mixed-class reconstruction, independent
  `va_copy`, repeated traversal, and hidden returns;
- exact generated-symbol bindings, relocation closure, non-executable stack,
  FDE coverage, `_Unwind_Backtrace` execution, and debugger backtraces across
  both bridge kinds;
- byte-identical bridge packaging from distinct working directories, with no
  assembler file symbol or build path;
- injected missing/failing packaging tools with destination preservation and
  complete temporary cleanup.
