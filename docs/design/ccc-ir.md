# CCC-IR invariants

CCC-IR is a typed, ABI-independent CFG representation between the typed AST
and Cranelift. The decision to have it is
[ADR-0001](../adr/0001-ccc-ir-middle-layer.md); target call details are a
separate immutable [module ABI plan](abi-and-varargs.md#module-abi-plan).

## Core invariants

- **Places vs values.** A place is an address expression plus type, qualifiers, and an optional bitfield descriptor. Every read is an explicit load and every write an explicit store; lvalue-to-rvalue conversion is never implicit.
- **Object identity and address-taking.** Address-taken, volatile, aggregate, atomic, and variably modified objects are materialized in memory. A pre-lowering scan classifies locals before any SSA value is emitted, so later `&local` cannot require retroactive materialization.
- **Pointer operations.** Scaled pointer arithmetic, pointer difference, array/member offsets, null values, and integer/pointer conversions are explicit operations with the source C rules attached. Codegen does not infer pointee size or signedness.
- **Aggregate value semantics.** Every aggregate rvalue is an immutable owned
  snapshot with compiler-managed backing storage. `AggregateSnapshot` observes
  its source once; `AggregateCopy` has C assignment semantics and remains
  correct when source and destination are identical or overlap through
  aliasing. `AggregateProject` derives a verifier-bounded field/index address
  into owned storage, including array decay from an aggregate rvalue. Lowering
  may use loads/stores, a temporary, `memmove`, or a proven-nonoverlapping
  `memcpy`; it cannot blindly call `memcpy`. Volatile aggregate accesses are
  expanded into ordered volatile accesses of the required width.
- **Variadic operations.** `VaStart`, `VaArg`, `VaCopy`, and `VaEnd` are
  ABI-neutral effectful instructions. `VaArg` records the requested canonical
  type; its immutable target fetch plan and control-flow expansion belong to
  `ccc-abi` and codegen respectively.
- **Bitfields.** A bitfield place carries storage unit, bit offset, width, signedness, and volatility/atomic restrictions. Layout is computed during semantic analysis by the shared `ccc-types` layout engine from the effective configuration.
- **Compound literals.** The IR creates an anonymous object with the correct block or static storage duration and makes initialization explicit.

## Memory effects

- **Volatile.** Each volatile access is marked non-elidable and non-movable. CCC passes use an explicit memory-effect model, and codegen uses Cranelift memory flags/barriers that preserve the same contract. A backend capability test checks emitted accesses because ordinary Cranelift loads/stores are not assumed to remain volatile across backend upgrades.
- **Atomics.** Atomic load, store, exchange, RMW, compare-exchange, thread fence, and signal fence carry a `MemoryOrder`; compare-exchange carries success and failure orders. Semantic analysis validates legal order combinations and maps `memory_order_consume` to acquire.
- **Calls and inline assembly.** Effects include read/write/unknown memory, returns-twice, noreturn, unwind behavior, and inline-assembly clobbers. Optimization cannot move memory operations across an unknown call, a returns-twice boundary, or a relevant assembly clobber.

Cranelift atomic instructions do not encode the complete C ordering contract. The backend adapter may conservatively lower supported operations to a stronger ordering. Unsupported widths, alignments, or target operations call target runtime helpers (`libatomic` or a CCC shim). `atomic_is_lock_free` and the `ATOMIC_*_LOCK_FREE` macros query that same backend/runtime capability table, so they cannot disagree with lowering. `atomic_signal_fence` must constrain compiler reordering; because CLIF has no compiler-only barrier, lowering may conservatively emit a hardware fence — stronger is permitted, elision is not.

## String literals and static storage

String-literal pooling is keyed by element type, encoding, complete code-unit sequence, alignment, and mutability mode—not raw bytes alone. Pooling is disabled when writable-string compatibility semantics would make object identity or mutation observable.

A static initializer is an `InitializerGraph`, not just a byte image. Its leaves are:

- zero fill or target-endian byte sequences;
- symbol-plus-addend relocations with an explicit relocation kind;
- repeated/aggregate fragments with required padding and alignment;
- addresses of functions, objects, string literals, TLS objects, and one-past locations where the target object format permits them.

Object emission resolves the graph into section data and relocations. Globals also carry linkage, visibility, section, TLS model, alignment, tentative-definition state, and common-vs-definition policy. `-fcommon` is an effective ABI/codegen option; the default is represented explicitly rather than inferred in the object writer.

## Guarantees from semantic analysis

Before lowering, semantic analysis guarantees:

- names, tags, linkage, storage duration, and type qualifiers are resolved;
- every expression has a type and every implicit conversion is explicit;
- complete object types have a known target layout, VLA extents remain explicit runtime values, and legal incomplete types remain marked incomplete;
- an operation that requires a complete or constant-sized type has already been checked;
- constant expressions required by C have been evaluated without erasing relocation-bearing address constants;
- declarations that require an unavailable target/backend capability have a diagnostic path and cannot reach silent fallback codegen.

These guarantees keep type inference out of CCC-IR, `ccc-abi`, and codegen without pretending that every legal C type has a compile-time size.
