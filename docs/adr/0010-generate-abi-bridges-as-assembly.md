# ADR-0010 — Generate ABI bridges as assembly

Status: accepted (2026-07-14)

## Context

Some supported C boundaries require machine-level ABI state that the pinned
Cranelift interface cannot express directly. Variadic calls on SysV AMD64, for
example, must set `%al`, construct the overflow argument area, and preserve
unwind information across an indirect target call.

`cranelift_module::Module::define_function_bytes` can place encoded function
bytes and relocations in the primary object, but it does not provide a
structured interface for the required call-frame information, arbitrary
section data, or their relocations. Using it would make CCC own an x86-64
encoder, branch relaxation, DWARF call-frame serialization, and
assembler-compatible symbol metadata.

## Decision

Render deterministic target assembly for generated ABI bridges. Assemble it
through the resolved target compiler driver, combine it with the Cranelift
object through a driver-mediated relocatable link, and localize only the exact
generated-symbol allowlist with a format-native symbol localizer. ELF uses a
compatible `objcopy --localize-symbols`; Mach-O uses Apple's `nmedit -R`, which
preserves symbol-indexed relocations while changing bindings.

Generated assembly carries explicit `.cfi_*`, symbol type and visibility
directives, and `.note.GNU-stack`. It intentionally omits `.file` and `.loc`:
common assemblers derive a debug-line directory from their working directory,
which would leak build paths and make otherwise identical artifacts differ.
Generated bridges therefore support unwinding but have no source-level line
mapping. The compiler rejects file symbols and debug sections in intermediate
bridge objects, then verifies the final relocatable object before atomic
publication.

## Alternatives

- **Insert raw function bytes.** This removes external assembly but makes CCC
  responsible for instruction encoding and object metadata without a
  maintained abstraction for either.
- **Invoke a raw assembler and linker.** This duplicates target, emulation,
  plugin, and sysroot selection already owned by the resolved compiler driver.
- **Localize every hidden symbol.** This can change user-defined hidden symbols
  that must remain resolvable across objects.

## Consequences

- Compilations that require generated bridges also require a matching compiler
  driver and compatible format-native symbol localizer.
- Bridge-free object emission remains independent of those tools.
- Assembly and object inspection become stable, auditable correctness gates.
- Generated symbols use collision-resistant deterministic names and only the
  manifest's exact private-symbol set is localized.

## Revisit if

Cranelift or another maintained component exposes instruction encoding together
with first-class unwind, custom-section, symbol, and relocation support
sufficient to express the complete bridge contract.
