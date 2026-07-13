# Compiler resource directory, headers, and runtime

The resource directory contains compiler-owned headers, hosted wrappers, target bridge templates, and CCC runtime shims. Its version and target capability manifest are checked against the compiler binary; mixing incompatible installations is a hard error.

## Header ownership

Headers are classified rather than all being treated as complete CCC replacements:

- **Compiler-owned:** `stdarg.h` and compiler builtin internals whose representation must match `ccc-abi`; small standard spelling headers such as `stdbool.h`, `stdalign.h`, and `stdnoreturn.h` where no libc ABI is involved.
- **Target-derived compiler headers:** `stddef.h`, `float.h`, and `stdatomic.h`, generated or selected from the effective configuration and backend/runtime capability table.
- **Hosted wrappers:** `stdint.h`, `limits.h`, and any platform header for which libc owns public typedefs, feature-test integration, or ABI declarations. A wrapper supplies compiler builtins and uses `#include_next` when the resolved libc header is authoritative.

Every wrapper is tested against each supported libc. CCC does not place a generic header ahead of the system tree if doing so changes libc typedefs or feature-test behavior. Freestanding mode uses self-contained target-derived variants and does not pretend hosted libc declarations are available.

`stdatomic.h` reports lock-free properties from the same table used by codegen. `stdarg.h` uses the target's actual `va_list` spelling and builtin operations. `float.h` follows the selected native or explicit compatibility long-double mode. When complex support is unavailable, the configuration defines `__STDC_NO_COMPLEX__` and the complex wrapper fails clearly rather than exposing unusable declarations.

## Include search

Search lists are ordered directory entries with provenance and class, not a flattened set of strings:

- quoted include: including-file directory, `-iquote`, `-I`, `-isystem`, compiler resource entries, resolved target system entries, `-idirafter`;
- angled include: the same without the including-file directory and `-iquote`;
- Darwin framework entries from `-F`/`-iframework` participate according to the selected driver/SDK rules.

`#include_next` resumes strictly after the directory entry that found the current header, even when another entry names the same physical directory. `__has_include` uses the same resolver transaction and does not mutate include state.

Header identity for `#pragma once` and cycle diagnostics uses stable filesystem identity (`device,inode` where available) with a normalized-realpath fallback. Diagnostics retain both the spelled and resolved path. Ordinary include guards remain macro semantics and do not depend on path canonicalization. The resolver handles symlinks, case sensitivity, missing files, permission errors, and include cycles deterministically.

Controls:

- `-nostdinc` removes compiler and system default entries but retains explicit user entries;
- `-nobuiltininc` removes compiler resource headers only;
- `--sysroot`/`-isysroot` are interpreted according to the resolved driver flavor and affect preprocessing and linking consistently;
- dependency output records the resolved file while preserving the requested path spelling required by the selected dependency option.

System include directories are obtained from the resolved target toolchain/sysroot and fingerprinted in the effective configuration. CCC never reuses host include paths for a cross target.

## Runtime and generated bridges

The resource directory contains versioned target objects or assembly templates for ABI bridges, long-double boundaries, stack probes, and helpers not supplied with the required ABI by the selected toolchain. The [runtime helper manifest](toolchain.md#runtime-helper-manifest) chooses the provider for every symbol. Generated bridge objects are placed in per-compilation temporary directories and are linked in deterministic order; their target, ABI mode, and build ID must match the compilation.
