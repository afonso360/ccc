# ABI lowering and variadic functions

This document defines how C calls cross the CCC-IR/Cranelift boundary. CCC-IR retains the source-level C signature; target-specific decisions are explicit data produced by `ccc-abi` and consumed by code generation.

## ABI plans

`ccc-abi` produces an immutable `AbiPlan` for every function definition and call site. It does not rewrite CCC-IR in place. A plan records:

- the source C signature and effective calling convention;
- the lowered scalar signature accepted by Cranelift;
- argument and return classifications, including register classes, stack offsets, extension rules, aggregate pieces, hidden `sret`, and copy-in/copy-out actions;
- whether the call is direct or indirect and the symbol/visibility/relocation requirements;
- variadic fixed-argument count, default promotions, target `va_list` layout, and any required bridge;
- the exact [`EffectiveCompilationConfig`](targets.md#effective-compilation-configuration) used to compute it.

The plan is serializable in `--dump-abi` output and is part of ABI snapshot tests. Optimizations may inspect the source signature but cannot change an `AbiPlan`; any transformation that changes a call signature must request a new plan.

## Aggregate calls

Each target classifier implements its published psABI, including SysV eightbytes, AAPCS64 HFA/HVA rules, RISC-V flattening, stack alignment, and hidden returns. An aggregate passed indirectly is copied into ABI-owned storage with the alignment and aliasing behavior specified by the plan. Cross-linked caller/callee tests are the authority; matching CCC on both sides is insufficient evidence.

## Variadic calls

Semantic analysis applies the C default argument promotions and retains the promoted type of every variadic actual argument. `ccc-abi` classifies the complete actual argument list even for an indirect call.

Cranelift signatures do not currently express C variadic semantics. CCC therefore has two interchangeable backend capabilities:

1. **Native capability:** a pinned Cranelift version can express the target's variadic call and callee-entry rules, including fixed-argument count and special registers.
2. **Generated bridge:** `ccc-abi` emits a `VarArgBridgePlan`; `ccc-link` assembles a target-specific bridge object. A call bridge receives a callee address plus a packed argument/return area, reconstructs the psABI register and stack state, performs the call, and stores the return value. This uniform form supports both direct and indirect calls without smuggling the callee through a global or changing argument positions.

No pinned Cranelift release currently provides the native capability, so until an upgrade passes the gates every variadic call and definition takes the bridge path — the bridges sit on the critical path of the first ABI milestone, not in a rarely exercised fallback corner.

For SysV AMD64 the bridge sets `%al` to the required upper bound on used vector argument registers. For Darwin arm64 it places unnamed arguments according to Apple's stack rules. AArch64 Linux and RISC-V bridges follow their own psABI rules; no target inherits another target's workaround.

If neither capability exists for the selected target and argument shape, declarations remain parseable but a variadic call is a hard, target-specific diagnostic. CCC never accepts only integer variadic calls while implying that general C variadics work.

## Variadic definitions and `va_list`

A variadic definition needs more than call-site shaping. When native backend support is unavailable, CCC emits an externally visible assembly entry bridge and an internal non-variadic CLIF body:

- the entry bridge receives the platform C ABI state, saves the required general-purpose and floating-point argument registers, identifies the overflow stack area, and constructs a target-specific `VaState`;
- it calls the internal body with the fixed parameters plus a hidden `VaState*`;
- `va_start` initializes the public target `va_list` from that state; `va_arg`, `va_copy`, and `va_end` operate on the psABI layout and enforce type/alignment rules;
- the bridge marshals the internal return value back through the platform ABI.

Calls within the same translation unit still use the externally valid entry point unless an optimization proves an equivalent ABI-preserving path. `va_list` is never represented by a single generic pointer: its type, array/struct spelling, register offsets, and overflow-area rules come from the target specification and must match the shipped `stdarg.h`.

Bridge and shim assembly is a first-class deliverable: every call, entry, and long-double bridge carries hand-written CFI/unwind records and minimal debug information so unwinding, `backtrace`, and debugger stepping work through it.

## Required validation

- classifier unit tests generated over scalar and aggregate shapes;
- CCC caller/reference callee and reference caller/CCC callee tests in both directions;
- direct and indirect variadic calls with zero and many unnamed arguments;
- integer, promoted floating-point, pointer, vector where supported, and aggregate `va_arg` cases;
- exhaustion boundaries between registers and stack, alignment holes, nested `va_list`, `va_copy`, and repeated traversal;
- disassembly assertions for target-only requirements such as SysV `%al`;
- unwind/backtrace and debugger stepping through call and entry bridges;
- a hard-error test for every target/configuration without a complete native or bridge capability.
