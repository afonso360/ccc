# Optimization contract

CCC separates source-language cleanup from machine-code optimization. The
frontend always performs mandatory scalar SSA promotion while lowering typed C
into CCC-IR. Optional CCC-IR passes run after lowering and before ABI planning;
Cranelift then optimizes the selected target's lowered machine representation.
This ordering means the immutable ABI plan and its digest always describe the
final CCC-IR consumed by code generation.

## Driver profiles

Optimization options are last-option-wins. Bare `-O` is `-O1`; omitting an
option is equivalent to `-O0`.

| Driver spelling | CCC-IR pass set | Cranelift setting |
| --- | --- | --- |
| default, `-O0` | Mandatory lowering, scalar SSA promotion, and verification only | `none` |
| `-O`, `-O1` | Redundant block parameters and copies, target-aware integer/conversion folding, constant CFG cleanup, unreachable and empty-block removal, conservative pure DCE | `speed` |
| `-O2`, `-O3` | The `-O1` set plus basic-block-local pure scalar/address CSE | `speed` |
| `-Os`, `-Oz` | The enabled shrink-only CCC-IR set, including local CSE | `speed_and_size` |

`-O3` currently has the same pass set as `-O2`, and `-Oz` has the same pass set
as `-Os`. A spelling is split only when a separately tested transformation
justifies a distinct contract. Optimization level is deliberately excluded
from ABI compatibility: objects compiled at different levels may be linked
together.

The predefined macro contract follows the same resolved profile.
`__OPTIMIZE__` is defined for every enabled profile and
`__OPTIMIZE_SIZE__` is additionally defined for `-Os` and `-Oz`. CCC does not
define fast-math, strict-aliasing, or inlining macros because those behaviors
are not part of this contract.

## CCC-IR ownership

CCC performs only transformations that need the typed C model or make the
verified, pre-ABI IR canonical:

- integer and conversion folding uses the effective target's exact C widths,
  signedness, and implementation-defined boundaries;
- block-parameter cleanup and CFG forwarding simplify the representation that
  the verifier and ABI digest observe;
- DCE consults an exhaustive instruction-effect classifier;
- CSE is restricted to pure scalar and address expressions within one basic
  block, and never reasons through memory or runtime-allocation epochs.

Every optimization entry verifies the module before and after the pass
pipeline. Cheap passes iterate to a function-size-derived hard bound, compact
IDs deterministically, preserve retained source spans, and reach an idempotent
form. Volatile or atomic accesses, fences, calls, returns-twice boundaries,
inline-assembly effects, aggregate observations and copies, computed-goto
targets, and runtime-size/allocation/trap effects are retained.

Floating-point reassociation, global value numbering, alias analysis, loop
transforms, instruction combining, register allocation, scheduling, and
target-specific peepholes remain Cranelift responsibilities. Reimplementing
those operations in CCC-IR would add a second machine optimizer without adding
C-language correctness information.

## Backend inlining

Code generation prepares and verifies every frontend-finalized CLIF definition
before compiling any one function. A target-invocation-local map resolves
namespace-zero Cranelift `FuncRef`s to exact-signature bodies, and
`Context::inline` performs the transformation before the ordinary
`ObjectModule::define_function` path. Candidates are not legalized or
pre-optimized separately, and CCC does not add CFG cleanup, GVN, peepholes, or
other substitutes for Cranelift's normal passes. `--emit-clif` consequently
shows the post-inlining function that enters that pipeline.

The first policy is deliberately narrow. At `-O2` and `-O3`, it considers
strong internal, native-boundary, non-recursive leaf definitions only. It
rejects bridge bodies, imports, weak or externally linked definitions,
returns-twice callers and callees, indirect or patchable call sites, signature
mismatches, user-named global storage, and exact `noinline` definitions.
Current Cranelift copies symbolic global values while inlining without remapping
their function-local user-name references, so CCC keeps those candidates out
instead of adding a duplicate backend remapper. The raw-CLIF limits are 24
instructions (32 when the C `inline` specifier supplies a hint), four blocks,
eight sites per caller, 96 estimated instructions of caller growth, and 16
estimated blocks of caller growth. Traversal is depth one
(`visit_callee=false`), and every original definition is still emitted as an
out-of-line symbol.

A safe internal leaf marked `always_inline` is required at every optimization
level and ignores the heuristic size and growth limits. If its direct call
cannot satisfy the initial safety contract, code generation reports `CCC4012`
instead of silently retaining the call. Exact `noinline` always wins; semantic
analysis rejects a declaration that combines the two attributes before code
generation.

Heuristic inlining is disabled when source debug information is requested
because CCC does not yet emit inlined-subroutine DIEs and abstract origins. A
required `always_inline` call in that mode receives `CCC4012` for the same
reason, so emitted DWARF never pretends that a transformed call is still an
ordinary out-of-line frame.

## Validation

Pass-local tests cover target-width wrapping and undefined or
implementation-defined boundaries, profile differentiation, empty-block
forwarding, deterministic fixed points, effect retention, and verifier
mutations. Driver tests require default output to equal `-O0`, equivalent
spellings to share output, representative input to change under optimization,
and mixed-profile objects to link. The execution catalog runs under `-O0`,
`-O2`, and `-Oz`; target oracles exercise their runtime and ABI matrices under
`-O0` and `-O2`; Csmith compares CCC at `-O0`, `-O2`, and `-Oz` with one
GCC/Clang reference consensus.
