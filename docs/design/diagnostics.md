# Diagnostics and error recovery

Diagnostics are a frontend and capability contract, not a rendering layer. `ccc-session` owns source files, interned spans, and expansion/include provenance, and carries the [effective compilation configuration](targets.md#effective-compilation-configuration) constructed by the driver; `ccc-diag` owns diagnostic codes, categories, structured messages, and renderers.

The driver owns diagnostic policy. The preprocessor emits warnings and
recoverable errors through a driver-provided sink instead of encoding warnings
in its success/error return type. The driver then applies warning options,
`-Werror`, source-ordered diagnostic pragma state, system-header suppression,
and the error limit through the shared diagnostic engine. A component may
return partial observable output for diagnostics, but parsing, object emission,
and atomic dependency-file replacement require an error-free preprocessing
state.

## Requirements

- **Spans everywhere.** Tokens and syntax/semantic/IR operations retain a compact source-origin ID. Generated operations may point to a primary source construct plus secondary origins rather than inventing a byte range.
- **Spelling and expansion locations.** Macro tokens retain spelling, expansion, argument-substitution, stringization, and token-paste provenance. Diagnostics can show a bounded macro backtrace and the include stack.
- **Occurrence-aware system state.** Every origin records the include
  occurrence and whether the token came from a system-header region. A later
  pragma cannot retroactively alter an earlier token's warning behavior.
- **Parser recovery.** Recovery synchronizes at grammar-aware declaration/statement boundaries, records inserted/skipped tokens, and suppresses dependent cascades without hiding independent errors.
- **Stable identity.** Every diagnostic has a stable code and category. Tests pin the code, severity, primary/secondary spans, and essential message; renderers may improve layout without breaking machine consumers.
- **Warning control.** `-W`, `-Wno-`, `-Werror`, per-category promotion, system-header suppression, and command-line provenance are handled consistently. Driver warning names come from an explicit registry; unknown names, including misspelled `-Werror=name` options, are rejected rather than ignored. Category switches are resolved in source order, and category-specific promotion or demotion takes precedence over global `-Werror`. Capability errors that protect ABI or semantics cannot be downgraded to warnings.
- **Error limits.** A configurable error limit stops semantic work safely while preserving already-emitted diagnostics.
- **Machine output.** JSON diagnostics use versioned schemas and resolved/spelled paths without leaking nondeterministic temporary directories.

Unsupported features identify the missing target/backend capability and, where applicable, the explicit compatibility option. They are emitted at semantic use rather than after partial object generation.

`#pragma GCC diagnostic push`, `pop`, `ignored`, `warning`, and `error` are
recognized as ordered warning-control events. The preprocessor applies them to
its own subsequent diagnostics and preserves them for later consumers.

## Parser recovery contract

`ccc-syntax` exposes `parse_recovering` and `parse_recovering_with_mode`. Each
returns a `RecoveringParse` containing a partial translation unit and every
independent `ParseError`. `parse` and `parse_with_mode` remain compatibility
wrappers: they run the same recovery machinery and return the first error.

Recovery checkpoints the ordinary-identifier classification environment before
each external declaration and block item. A failed item restores that snapshot,
including typedef-name bindings and scope events. The parser then synchronizes
at an external-declaration, block-declaration, or statement boundary. A missing
semicolon at an unambiguous next-item boundary is represented as an inserted
token; otherwise skipped tokens are represented by one source span. This
metadata is carried by `ParseError::recovery`. Only one parser diagnostic is
retained for a failed boundary, so tokens discarded by that recovery do not
produce follow-on parser errors.

Every C statement and expression starter participates in missing-semicolon
recovery. Declaration recovery may therefore stop before a following statement,
not only before another declaration. At file scope, synchronization distinguishes
a function body from braces nested in a declaration or initializer, so the
terminating semicolon of an invalid declaration is consumed in every language
mode.

The driver passes the partial translation unit to semantic analysis when the
error limit still permits work. Consequently a semantic error in a retained,
independent declaration can be reported alongside parser errors, while a
malformed AST node is never fabricated for semantic analysis.
Identifiers introduced by a failed declaration are retained separately as
scope-bounded poison bindings. They preserve recovery ordering without entering
the partial AST: semantic analysis uses them only to suppress errors dependent
on the failed declaration, while independent errors and uses outside the
binding's lexical scope remain diagnosable.

## Compilation-wide policy

Preprocessing, parsing, and semantic analysis emit into one `DiagnosticEngine`
in source-stage order. `-ferror-limit=N` therefore counts errors promoted by
`-Werror`, preprocessing errors, parser errors, and semantic errors together.
The default is 20; `-ferror-limit=0` disables the limit. On reaching a nonzero
limit, the engine appends `CCC0000`, preserves all diagnostics already emitted,
and prevents later frontend work.
The preprocessor consults the engine after each source line, and parsing and
semantic analysis receive the remaining error budget. Reaching the limit thus
stops the current stage as well as preventing subsequent stages.

Any error makes the compilation fail closed. Object packaging and side-effect
dependency publication are deferred until frontend and code-generation
diagnostics have succeeded. Dependency-only and preprocessing actions perform
the same check before replacing their destination. An existing object or
dependency file is therefore left unchanged after an error.
Assembly output follows the same pending-output protocol: the external
assembler writes a private path that replaces the requested object only after a
successful exit.

## Output formats

Text output is the default. `-fdiagnostics-format=text` selects it explicitly;
`-fdiagnostics-format=json` writes one JSON document followed by a newline.
Per-source compilation and replayable `-###` commands preserve a requested JSON
format.
Driver, target, hardening, code-generation, and publication diagnostics are
composed with frontend diagnostics before rendering. A diagnostic invocation
therefore emits one JSON document rather than concatenating JSON and text or
multiple JSON documents.

The JSON document has this top-level shape:

```json
{"schema_version":1,"diagnostics":[]}
```

Version 1 uses a fixed field order and preserves diagnostic, secondary-span,
note, include-frame, and macro-frame insertion order. Each diagnostic contains:

- `severity`, `code`, `category`, and `message`;
- `primary`, either `null` or a labeled source annotation;
- `secondary`, an ordered array of labeled source annotations;
- `notes`, an ordered string array;
- `include_trace` and `macro_trace`, each containing `truncated` and `frames`.

Every source annotation contains a `location` with `spelled_path`,
`resolved_path`, `display_path`, and half-open `start`/`end` positions. A
position records its physical byte offset and its presumed one-based line and
column. This keeps command-line spelling, resolved include identity, and
`#line` presentation distinct. Callers that need reproducible snapshots pass
stable spelled and resolved paths rather than copying compiler-created
temporary names into the source map.

Include frames are locations of the introducing directives. Macro frames have
a stable `kind` and the applicable invocation, definition, argument,
replacement, operator, left, or right locations. The default renderer retains
at most 32 include frames and 8 macro-origin frames; `truncated` is true when
additional provenance exists. JSON string escaping follows the JSON control
character rules, including deterministic lowercase `\u` escapes.
