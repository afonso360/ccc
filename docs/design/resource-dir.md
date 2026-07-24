# Compiler resource directory, headers, and runtime

The resource directory contains compiler-owned headers, hosted wrappers, and
optional CCC runtime shims. Its version and target capability manifest are
checked against the compiler binary; mixing incompatible installations is a
hard error.

The GNU compatibility tuple, C11 identity, CCC identity, target facts, dynamic
predicates, and later feature macros together form the
[effective compiler identity](core-c11-and-gnu-semantics.md#effective-compiler-identity).
The tuple alone is not historical GCC emulation. A resource-manifest revision
must enumerate any predicate or macro addition that can select a different
hosted-header path.

## Header ownership

Headers are classified rather than all being treated as complete CCC replacements:

- **Compiler-owned:** target-invariant compiler interface text such as
  `stdarg.h` and `stdatomic.h`, which delegate representation or operations to
  target-aware compiler builtins; small standard spelling headers such as
  `stdbool.h`, `stdalign.h`, and `stdnoreturn.h` where no libc ABI is involved.
- **Target-derived compiler headers:** `stddef.h` and `float.h`, generated or selected from the effective configuration.
- **Hosted wrappers:** `math.h`, `stdint.h`, `limits.h`, and any platform header for which libc owns public typedefs, feature-test integration, or ABI declarations. A wrapper supplies compiler builtins and uses `#include_next` when the resolved libc header is authoritative. The shipped `math.h` wrapper retains libc declarations and constants while replacing only the `float`/`double` classification macros that would otherwise expose unselected `long double` branches. On Apple arm64, the `sys/cdefs.h` wrapper selects the SDK's documented static-inline fallback for header implementation functions. CCC's first inlining policy enforces safe translation-unit-local leaf cases, but it does not yet certify the SDK's complete external-inline, non-leaf, and debug-information contract; public declarations and ABI types continue to come from the SDK.

Every wrapper is tested against each supported libc. CCC does not place a generic header ahead of the system tree if doing so changes libc typedefs or feature-test behavior. Freestanding mode uses self-contained target-derived variants and does not pretend hosted libc declarations are available.

The versioned resource manifest assigns every shipped header to exactly one
ownership class: `compiler_owned`, `target_derived`, or `hosted_wrappers`. Its
complete inventory is validated before the directory enters include search.
Duplicate classification, unlisted files, missing files, and normalized-path
violations are hard installation errors. A manifest format change is rejected
rather than interpreted using an older ownership model.

The target-derived `<stddef.h>` template uses `__SIZE_TYPE__`,
`__PTRDIFF_TYPE__`, and `__WCHAR_TYPE__` from the effective configuration. Its
`max_align_t` combines the target's fundamental `long long` and `long double`
alignment requirements, and `offsetof` delegates to `__builtin_offsetof` so
semantic analysis and the ABI layout oracle share one member-offset answer.
It implements the conventional `__need_*` partial-include protocol used by
hosted headers. The associated parser and builtin requirements are part of the
[frontend capability contract](frontend-capabilities.md).

`stdatomic.h` exposes the native fundamental integer/pointer subset and maps its
operations to registry-gated compiler builtins. Its lock-free query applies the
same type/width/alignment gate as semantic lowering. The complete atomic-type
capability remains denied while aggregate, floating, wide, and weakened-alignment
forms have no runtime-helper fallback.
`stdarg.h` aliases the reserved `__builtin_va_list` spelling and maps the
standard operations to compiler builtins; its source does not embed a target
record layout. The canonical builtin type supplies the target's actual
array-of-one representation. Native `long double` predefined macros come from
the same target layout used by semantic analysis; the target toolchain owns its
hosted `float.h`. When complex support is unavailable, the configuration
defines `__STDC_NO_COMPLEX__` and the complex wrapper fails clearly rather than
exposing unusable declarations.

## Include search

Search lists are ordered directory entries with provenance and class, not a flattened set of strings:

- quoted include: including-file directory, `-iquote`, `-I`, `-isystem`, compiler resource entries, resolved target system entries, `-idirafter`;
- angled include: the same without the including-file directory and `-iquote`;
- Darwin framework entries from `-F`/`-iframework` participate according to the selected driver/SDK rules.

`#include_next` and `__has_include_next` resume strictly after the directory
entry that found the current header, even when another entry names the same
physical directory. Header predicates use the same resolver transaction as
directives and do not mutate include state. A direct quoted or angled
predicate operand is resolved as written; a computed operand is macro-expanded
and must then form exactly one valid header name.

When an `#include` operand is not already a direct quoted or angled header
name, its tokens are macro-expanded and the result must form exactly one valid
header name. The same resolver handles direct includes, computed includes,
`#include_next`, forced inputs, and `__has_include`; no caller reconstructs a
parallel search algorithm.

Header identity for `#pragma once` and cycle diagnostics uses stable filesystem
identity (`device,inode` where available) with a normalized-realpath fallback.
Diagnostics retain both the spelled and resolved path. Ordinary include guards
and terminating recursive-inclusion idioms remain macro semantics: a repeated
active identity is not rejected by itself. If recursion reaches the configured
include-depth limit, the diagnostic reports the shortest repeated-identity
cycle visible in the active stack. The resolver handles symlinks, case
sensitivity, missing files, and permission errors deterministically.

Each include occurrence records its parent, the search entry that found it,
and whether it is a system header. `#pragma GCC system_header` changes the
system-header state for subsequent tokens in the current occurrence. Origin
records snapshot that state so warning suppression, linemarkers, and
`-MM`/`-MMD` filtering cannot disagree.

Controls:

- `-nostdinc` removes compiler and system default entries but retains explicit user entries;
- `-nobuiltininc` removes compiler resource headers only;
- `--sysroot`/`-isysroot` are interpreted according to the resolved driver flavor and affect preprocessing and linking consistently;
- dependency output records the resolved file while preserving the requested path spelling required by the selected dependency option.

System include directories are obtained from the resolved target toolchain/sysroot and fingerprinted in the effective configuration. CCC never reuses host include paths for a cross target.

## Runtime and generated bridges

Versioned runtime shims may be resource-owned when a target operation is not
provided with the required ABI by the selected toolchain. The
[runtime helper manifest](toolchain.md#runtime-helper-manifest) chooses the
provider for each such symbol. The hosted scoped-arena provider is different:
its allocation and cleanup logic lowers directly into each affected function,
with ordinary `realloc` and `free` imports outside the compiler-builtins helper
manifest. Runtime layout operations that allocate no automatic object do not
import that provider. It does not require a resource-owned runtime object. A
freestanding arena provider may be resource-owned and is enabled only for the
exact targets listed by its provider contract. ABI bridge assembly is not a resource template:
`ccc-link` renders it from the verified `ModuleAbiPlan` for each compilation,
then assembles, partially links, and exactly localizes it as specified by
[ADR-0010](../adr/0010-generate-abi-bridges-as-assembly.md).
