# ADR-0011 — Back runtime-sized automatic objects with a scoped arena

Status: accepted (2026-07-15)

## Context

C variable-length array objects have automatic storage duration, runtime
extents, and declaration-sensitive lifetimes. The C abstract machine requires
their storage to remain reserved for that lifetime, but does not require a
machine-stack address.

The pinned Cranelift backend lays out fixed stack slots and register-allocation
spills relative to a stack pointer that remains stable after frame setup. If an
ordinary callee or assembly thunk returned with the caller's stack pointer
changed, later slot and spill accesses would address the wrong memory. A common
function exit would restore the stack too late to make those intervening
accesses safe.

GNU `__builtin_alloca` has a stronger, separate contract: it allocates on the
calling function's native stack and ordinarily retains the allocation until
that function returns. ISO VLA support must not imply that extension.

## Decision

CCC-IR represents runtime-sized automatic storage independently of its physical
provider. It models declaration-time allocation as an explicit effect and does
not expose an allocator, arena layout, or native stack token in the IR.

The hosted provider selected by this decision is a per-invocation arena with
one cached allocation per syntactic runtime-sized declaration. Each cache's
base and capacity are held in a fixed Cranelift stack slot; generated code never
moves the native stack pointer. Each runtime-sized declaration:

1. evaluates and saves every runtime extent exactly once;
2. rejects nonpositive extents;
3. computes strides and total byte size with checked `size_t` arithmetic;
4. reuses retained capacity or grows it with the hosted `realloc` provider;
5. produces a stable pointer at the effective target alignment; and
6. releases every cached base on ordinary function return.

The effective alignment is the greater of the declared alignment and the
target's VLA minimum. The System V AMD64 provider therefore guarantees at least
16-byte alignment and also honors stronger `_Alignas` requirements. Extent,
alignment, or allocation failure enters a non-returning failure path; it never
produces a null or undersized VLA object.

Semantic analysis records the active variably modified declaration path at
labels and switch targets. It rejects control-flow ingress that bypasses a
declaration. Lowering restores the active suffix for normal fallthrough,
`break`, `continue`, outward or backward `goto`, and return. A return operand is
fully evaluated and copied before restoration. When computed goto is enabled
alongside this provider, its dispatch must perform the same verified restoration
and reject targets that enter a region.

The logical version-one provider surface is the target profile's exact hosted
`realloc(void *, size_t)` and `free(void *)` ABI. A generated object linked by an
external GCC- or Clang-compatible driver therefore needs only its ordinary
hosted libc dependencies; it does not require a separately installed CCC
runtime library or generated assembly. Freestanding profiles require another
descriptor with a manifest-selected allocator or keep runtime-sized object
storage unavailable.

The arena is per invocation, not a thread-local secondary stack. That keeps
recursion and concurrent calls independent and prevents a nonlocal exit from
corrupting shared active-region state. A thread-local cache may recycle fully
detached chunks only when active marks remain invocation-owned and the revised
provider passes the performance and nonlocal-exit evidence.

`__builtin_alloca`, related native-stack builtins, stack-usage reporting, and
dynamic stack-clash probing remain unavailable until a maintained backend
operation participates in frame layout, spills, calls, probes, debug data, and
unwind information. CCC will pursue that operation upstream rather than carry a
private Cranelift fork.

A nonlocal exit may strand chunks owned by an abandoned invocation, as C11
explicitly permits VLA storage to be squandered by `longjmp`. A returns-twice
point in the same function still requires an explicit arena checkpoint; until
that interaction is verified, semantic analysis diagnoses functions that
combine the two capabilities. Cross-language unwinding requires cleanup
integration before it may cross an active arena.

The hosted arena provider is not POSIX async-signal-safe because it depends on
the hosted allocator. This is a documented provider restriction that cannot be
reliably diagnosed from a signal-handler declaration. A profile requiring
async-signal-safe runtime-sized storage must use a proven provider or leave VLA
object storage unavailable.

A VLA declared inside a GNU statement expression reaches the end of its C
lifetime at the closing brace after the final subexpression value is captured.
Retained physical arena capacity does not extend that lifetime; a result that
refers to the object is invalid outside the statement expression.

## Alternatives

- **Fork Cranelift or carry a private dynamic-stack operation.** This could
  provide native stack semantics, but creates a backend maintenance and release
  burden before any current corpus requires it.
- **Wait for upstream native support.** This avoids a fork but needlessly ties
  ISO automatic-storage semantics to a physical placement the language does not
  require.
- **Use a stack-mutating helper or assembly thunk.** Returning with a changed
  caller stack pointer invalidates fixed slots and spills and is therefore
  unsound.
- **Outline allocation-active regions.** Continuation conversion can preserve
  each Cranelift frame, but capturing locals and representing every control-flow
  transfer is substantially larger than the storage capability.
- **Use an active thread-local secondary stack.** It can amortize hot calls, but
  nonlocal exit, signal reentrancy, and nested invocations make active ownership
  unsafe without a more complex runtime protocol.
- **Continue rejecting runtime-sized automatic objects.** This remains the
  behavior until every provider gate is implemented, but is not the selected
  long-term design.

## Consequences

- Cranelift's fixed-stack-pointer invariant remains intact.
- Runtime-sized objects have correct logical lifetime, alignment, stable active
  addresses, recursion behavior, and bounded loop reuse. Physical capacity can
  be retained until the invocation returns.
- Heap exhaustion replaces native stack exhaustion, and CCC makes no native
  stack observability or probing claim for the arena provider.
- The hosted provider is enabled for automatic VLA objects after semantic, IR,
  runtime, link, failure-path, leak, alignment, recursion, and thread tests.
  `__STDC_NO_VLA__` remains defined until the broader runtime-layout and
  variably modified type surface is complete.
- Provider-neutral CCC-IR permits a future verified native Cranelift operation
  without redesigning semantic analysis or control-flow cleanup.

## Revisit if

Cranelift gains a verified native dynamic-stack facility, async-signal-safe
allocation becomes required, nonlocal-exit interoperability cannot be bounded,
or measured allocator and code-size costs justify another provider.

## References

- [C11 committee draft N1570](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1570.pdf),
  especially 6.2.4, 6.8.6.1, and 7.13.2.1.
- [GCC variable-length array semantics](https://gcc.gnu.org/onlinedocs/gcc/Variable-Length.html).
- [GCC native stack-allocation builtin contract](https://gcc.gnu.org/onlinedocs/gcc/Stack-Allocation.html).
- [WG14 N3437, VLA allocation control](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n3437.htm),
  as evidence of allocator-backed implementation practice rather than adopted
  C11 wording.
