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
