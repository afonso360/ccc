# Diagnostics and error recovery

Diagnostics are a frontend and capability contract, not a rendering layer. `ccc-session` owns source files, interned spans, and expansion/include provenance, and carries the [effective compilation configuration](targets.md#effective-compilation-configuration) constructed by the driver; `ccc-diag` owns diagnostic codes, categories, structured messages, and renderers.

## Requirements

- **Spans everywhere.** Tokens and syntax/semantic/IR operations retain a compact source-origin ID. Generated operations may point to a primary source construct plus secondary origins rather than inventing a byte range.
- **Spelling and expansion locations.** Macro tokens retain spelling, expansion, argument-substitution, stringization, and token-paste provenance. Diagnostics can show a bounded macro backtrace and the include stack.
- **Parser recovery.** Recovery synchronizes at grammar-aware declaration/statement boundaries, records inserted/skipped tokens, and suppresses dependent cascades without hiding independent errors.
- **Stable identity.** Every diagnostic has a stable code and category. Tests pin the code, severity, primary/secondary spans, and essential message; renderers may improve layout without breaking machine consumers.
- **Warning control.** `-W`, `-Wno-`, `-Werror`, per-category promotion, system-header suppression, and command-line provenance are handled consistently. Capability errors that protect ABI or semantics cannot be downgraded to warnings.
- **Error limits.** A configurable error limit stops semantic work safely while preserving already-emitted diagnostics.
- **Machine output.** JSON diagnostics use versioned schemas and resolved/spelled paths without leaking nondeterministic temporary directories.

Unsupported features identify the missing target/backend capability and, where applicable, the explicit compatibility option. They are emitted at semantic use rather than after partial object generation.
