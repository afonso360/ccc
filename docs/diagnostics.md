# Diagnostic code registry

CCC diagnostic identifiers are stable serialized interfaces. Text diagnostics,
JSON output, tests, and external tooling continue to observe seven-byte
identifiers of the form `CCCdddd`.

The typed registry in `ccc-diag` is intentionally partial. It currently covers
four codes whose identity affects compiler control flow. Existing literal codes
that are not listed below remain valid while their producers are migrated in
small, reviewable groups; absence from this table does not retire or reassign a
code.

The component that owns a code controls its number and meaning. A different
component may propagate, inspect, or test that code without becoming its owner.
The registry can separately list another component as an allowed emitter when
an architectural boundary genuinely requires it.

## Owner bands

Ranges are inclusive. Unlisted ranges and gaps are reserved.

| Owner | Range |
| --- | --- |
| Diagnostics engine | `CCC0000` |
| Preprocessor | `CCC0001`-`CCC0009`, `CCC1000`-`CCC1009`, `CCC1100`-`CCC1399` |
| Syntax | `CCC1010`-`CCC1099` |
| Semantic analysis | `CCC2200`-`CCC2499` |
| IR | `CCC3100`-`CCC3199` |
| ABI | `CCC3500`-`CCC3599` |
| Code generation | `CCC4000`-`CCC4099` |
| Linking | `CCC5000`-`CCC5099` |
| Driver | `CCC6000`-`CCC6099` |

## Registered codes

Entries are kept in ascending numeric order.

| Code | Symbol | Owner | Allowed emitters | Meaning |
| --- | --- | --- | --- | --- |
| `CCC0000` | `diagnostics::TOO_MANY_ERRORS` | Diagnostics engine | Diagnostics engine | The compilation reached its configured error limit. |
| `CCC0002` | `preprocessor::UNTERMINATED_LITERAL` | Preprocessor | Preprocessor | A character or string literal is unterminated. |
| `CCC1315` | `preprocessor::WARNING_DIRECTIVE` | Preprocessor | Preprocessor | A `#warning` preprocessing directive was evaluated. |
| `CCC2274` | `semantic::UNDECLARED_IDENTIFIER` | Semantic analysis | Semantic analysis | An expression uses an undeclared identifier. |

## Validation and compatibility

The registry is the only constructor for typed diagnostic codes. Its
compile-time checks enforce the `CCCdddd` format, unique codes and metadata,
non-overlapping owner bands, owner-range membership, nonempty documentation
keys, and stable numeric ordering. Every owner is also required to be an
allowed emitter for its code. Generating and checking this Markdown table from
the complete registry remains part of the incremental migration.

Public diagnostic structures still store `String` or `&'static str` values.
Typed constants convert at those existing boundaries, preserving all text and
JSON serialization. Exact string assertions remain useful compatibility tests
throughout the incremental migration.
