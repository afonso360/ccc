# Diagnostic code registry

CCC diagnostic identifiers are stable serialized interfaces. Text diagnostics,
JSON output, tests, and external tooling continue to observe seven-byte
identifiers of the form `CCCdddd`.

The typed registry in `ccc-diag` is intentionally incremental. It covers every
production-owned diagnostics-engine, preprocessing, and syntax code, plus the
first migrated semantic-analysis code. Existing literal codes owned by later
compiler phases remain valid while their producers are migrated in small,
reviewable groups; absence from this table does not retire or reassign a code.

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

<!-- BEGIN GENERATED DIAGNOSTIC REGISTRY -->
| Code | Symbol | Owner | Allowed emitters | Documentation key | Meaning |
| --- | --- | --- | --- | --- | --- |
| `CCC0000` | `diagnostics::TOO_MANY_ERRORS` | diagnostics | diagnostics | `too-many-errors` | the compilation reached its configured error limit |
| `CCC0001` | `preprocessor::UNTERMINATED_BLOCK_COMMENT` | preprocessor | preprocessor | `unterminated-block-comment` | a block comment is unterminated |
| `CCC0002` | `preprocessor::UNTERMINATED_LITERAL` | preprocessor | preprocessor | `unterminated-literal` | a character or string literal is unterminated |
| `CCC0004` | `preprocessor::INVALID_IDENTIFIER_UCN` | preprocessor | preprocessor | `invalid-identifier-ucn` | an identifier contains an invalid universal character name |
| `CCC0005` | `preprocessor::UTF8_CHARACTER_CONSTANT` | preprocessor | preprocessor | `utf8-character-constant` | a character constant uses the invalid u8 prefix |
| `CCC0006` | `preprocessor::INVALID_CHARACTER_UCN` | preprocessor | preprocessor | `invalid-character-ucn` | a character constant contains an invalid universal character name |
| `CCC1001` | `preprocessor::TRIGRAPH` | preprocessor | preprocessor | `trigraph` | a trigraph was converted or ignored |
| `CCC1010` | `syntax::INVALID_NUMERIC_CONSTANT` | syntax | syntax | `invalid-numeric-constant` | a preprocessing number is not a valid numeric constant |
| `CCC1011` | `syntax::INVALID_STRING_LITERAL` | syntax | syntax | `invalid-string-literal` | a string literal cannot be decoded |
| `CCC1012` | `syntax::INCOMPATIBLE_STRING_LITERAL_CONCATENATION` | syntax | syntax | `incompatible-string-literal-concatenation` | adjacent string literals cannot be concatenated |
| `CCC1013` | `syntax::INVALID_CHARACTER_CONSTANT` | syntax | syntax | `invalid-character-constant` | a character constant cannot be decoded |
| `CCC1020` | `syntax::PARSE_ERROR` | syntax | syntax | `parse-error` | the token stream does not satisfy C syntax |
| `CCC1101` | `preprocessor::MACRO_EXPANSION_DEPTH_LIMIT` | preprocessor | preprocessor | `macro-expansion-depth-limit` | macro expansion exceeded its configured depth limit |
| `CCC1102` | `preprocessor::PREPROCESSING_TOKEN_LIMIT` | preprocessor | preprocessor | `preprocessing-token-limit` | macro expansion exceeded its configured token limit |
| `CCC1103` | `preprocessor::MACRO_ARGUMENT_DEPTH_LIMIT` | preprocessor | preprocessor | `macro-argument-depth-limit` | macro arguments exceeded their configured nesting limit |
| `CCC1104` | `preprocessor::MACRO_ARGUMENT_COUNT` | preprocessor | preprocessor | `macro-argument-count` | a macro invocation has the wrong number of arguments |
| `CCC1105` | `preprocessor::UNTERMINATED_MACRO_INVOCATION` | preprocessor | preprocessor | `unterminated-macro-invocation` | a function-like macro invocation is unterminated |
| `CCC1106` | `preprocessor::INVALID_STRINGIZE_OPERAND` | preprocessor | preprocessor | `invalid-stringize-operand` | the stringize operator does not precede a macro parameter |
| `CCC1107` | `preprocessor::INVALID_TOKEN_PASTE_POSITION` | preprocessor | preprocessor | `invalid-token-paste-position` | the token-paste operator appears without both operands |
| `CCC1108` | `preprocessor::INVALID_TOKEN_PASTE_RESULT` | preprocessor | preprocessor | `invalid-token-paste-result` | token pasting does not form one preprocessing token |
| `CCC1109` | `preprocessor::INVALID_PRAGMA_OPERATOR` | preprocessor | preprocessor | `invalid-pragma-operator` | a \_Pragma operator has an invalid operand |
| `CCC1110` | `preprocessor::MACRO_REDEFINED` | preprocessor | preprocessor | `macro-redefined` | a macro is redefined with a different replacement |
| `CCC1111` | `preprocessor::UNSUPPORTED_VA_OPT` | preprocessor | preprocessor | `unsupported-va-opt` | \_\_VA\_OPT\_\_ is unavailable in the selected compatibility profile |
| `CCC1112` | `preprocessor::VA_ARGS_OUTSIDE_VARIADIC_MACRO` | preprocessor | preprocessor | `va-args-outside-variadic-macro` | \_\_VA\_ARGS\_\_ appears outside a variadic macro replacement |
| `CCC1201` | `preprocessor::UNEXPECTED_CONDITIONAL_TOKEN` | preprocessor | preprocessor | `unexpected-conditional-token` | a conditional expression has an unexpected trailing token |
| `CCC1202` | `preprocessor::DEFINED_REQUIRES_IDENTIFIER` | preprocessor | preprocessor | `defined-requires-identifier` | the defined operator has no identifier operand |
| `CCC1203` | `preprocessor::DEFINED_MISSING_CLOSE` | preprocessor | preprocessor | `defined-missing-close` | the parenthesized defined operator is not closed |
| `CCC1204` | `preprocessor::PREDICATE_REQUIRES_PARENTHESES` | preprocessor | preprocessor | `predicate-requires-parentheses` | a conditional feature predicate is not parenthesized |
| `CCC1205` | `preprocessor::UNTERMINATED_PREDICATE` | preprocessor | preprocessor | `unterminated-predicate` | a conditional feature predicate is unterminated |
| `CCC1206` | `preprocessor::CONDITIONAL_MISSING_COLON` | preprocessor | preprocessor | `conditional-missing-colon` | a conditional expression is missing its colon |
| `CCC1207` | `preprocessor::SHIFT_COUNT_TOO_LARGE` | preprocessor | preprocessor | `shift-count-too-large` | a conditional-expression shift count is too large |
| `CCC1208` | `preprocessor::CONDITIONAL_DIVISION_BY_ZERO` | preprocessor | preprocessor | `conditional-division-by-zero` | a conditional expression divides by zero |
| `CCC1209` | `preprocessor::CONDITIONAL_MISSING_CLOSE` | preprocessor | preprocessor | `conditional-missing-close` | a parenthesized conditional expression is not closed |
| `CCC1210` | `preprocessor::CONDITIONAL_EXPECTED_EXPRESSION` | preprocessor | preprocessor | `conditional-expected-expression` | a conditional expression is missing an operand |
| `CCC1211` | `preprocessor::INVALID_CONDITIONAL_INTEGER` | preprocessor | preprocessor | `invalid-conditional-integer` | a conditional expression contains an invalid integer constant |
| `CCC1212` | `preprocessor::INVALID_CONDITIONAL_CHARACTER` | preprocessor | preprocessor | `invalid-conditional-character` | a conditional expression contains an invalid character constant |
| `CCC1213` | `preprocessor::INVALID_CONDITIONAL_TOKEN` | preprocessor | preprocessor | `invalid-conditional-token` | a conditional expression contains an invalid token |
| `CCC1301` | `preprocessor::INVALID_COMMAND_LINE_MACRO_NAME` | preprocessor | preprocessor | `invalid-command-line-macro-name` | a command-line macro has an invalid name |
| `CCC1302` | `preprocessor::FORCED_INPUT_UNAVAILABLE` | preprocessor | preprocessor | `forced-input-unavailable` | a forced preprocessing input cannot be opened or read |
| `CCC1303` | `preprocessor::INCLUDE_DEPTH_LIMIT` | preprocessor | preprocessor | `include-depth-limit` | header inclusion exceeded its configured depth limit |
| `CCC1304` | `preprocessor::UNKNOWN_SOURCE_OCCURRENCE` | preprocessor | preprocessor | `unknown-source-occurrence` | the preprocessor cannot resolve a source occurrence |
| `CCC1305` | `preprocessor::UNTERMINATED_CONDITIONAL_DIRECTIVE` | preprocessor | preprocessor | `unterminated-conditional-directive` | a conditional directive group is unterminated |
| `CCC1306` | `preprocessor::CONDITIONAL_REQUIRES_IDENTIFIER` | preprocessor | preprocessor | `conditional-requires-identifier` | an #ifdef or #ifndef directive does not name one identifier |
| `CCC1307` | `preprocessor::ELIF_WITHOUT_IF` | preprocessor | preprocessor | `elif-without-if` | an #elif directive has no matching #if |
| `CCC1308` | `preprocessor::ELIF_AFTER_ELSE` | preprocessor | preprocessor | `elif-after-else` | an #elif directive follows #else in the same group |
| `CCC1309` | `preprocessor::ELSE_WITHOUT_IF` | preprocessor | preprocessor | `else-without-if` | an #else directive has no matching #if |
| `CCC1310` | `preprocessor::TOKENS_AFTER_ELSE` | preprocessor | preprocessor | `tokens-after-else` | tokens follow an #else directive |
| `CCC1311` | `preprocessor::DUPLICATE_ELSE` | preprocessor | preprocessor | `duplicate-else` | a conditional group contains more than one #else |
| `CCC1312` | `preprocessor::TOKENS_AFTER_ENDIF` | preprocessor | preprocessor | `tokens-after-endif` | tokens follow an #endif directive |
| `CCC1313` | `preprocessor::ENDIF_WITHOUT_IF` | preprocessor | preprocessor | `endif-without-if` | an #endif directive has no matching #if |
| `CCC1314` | `preprocessor::ERROR_DIRECTIVE` | preprocessor | preprocessor | `error-directive` | an #error preprocessing directive was evaluated |
| `CCC1315` | `preprocessor::WARNING_DIRECTIVE` | preprocessor | preprocessor | `warning-directive` | a #warning preprocessing directive was evaluated |
| `CCC1316` | `preprocessor::UNKNOWN_DIRECTIVE` | preprocessor | preprocessor | `unknown-directive` | an unknown preprocessing directive was evaluated |
| `CCC1317` | `preprocessor::DEFINE_REQUIRES_NAME` | preprocessor | preprocessor | `define-requires-name` | a #define directive does not name a macro |
| `CCC1318` | `preprocessor::INVALID_VARIADIC_PARAMETER_POSITION` | preprocessor | preprocessor | `invalid-variadic-parameter-position` | a variadic macro parameter is not final |
| `CCC1319` | `preprocessor::EXPECTED_MACRO_PARAMETER` | preprocessor | preprocessor | `expected-macro-parameter` | a macro parameter list does not contain an identifier |
| `CCC1320` | `preprocessor::DUPLICATE_MACRO_PARAMETER` | preprocessor | preprocessor | `duplicate-macro-parameter` | a macro parameter name is duplicated |
| `CCC1321` | `preprocessor::INVALID_MACRO_PARAMETER_LIST` | preprocessor | preprocessor | `invalid-macro-parameter-list` | a macro parameter list has invalid punctuation |
| `CCC1322` | `preprocessor::INVALID_UNDEF` | preprocessor | preprocessor | `invalid-undef` | an #undef directive does not contain exactly one identifier |
| `CCC1323` | `preprocessor::INVALID_INCLUDE_OPERAND` | preprocessor | preprocessor | `invalid-include-operand` | an include operand does not form one header name |
| `CCC1324` | `preprocessor::HEADER_NOT_FOUND` | preprocessor | preprocessor | `header-not-found` | an included header cannot be found |
| `CCC1325` | `preprocessor::INVALID_LINE_NUMBER` | preprocessor | preprocessor | `invalid-line-number` | a #line directive has an invalid line number |
| `CCC1326` | `preprocessor::INVALID_LINE_FILE_NAME` | preprocessor | preprocessor | `invalid-line-file-name` | a #line file name is not a string literal |
| `CCC1327` | `preprocessor::TOKENS_AFTER_LINE` | preprocessor | preprocessor | `tokens-after-line` | tokens follow the operands of a #line directive |
| `CCC1328` | `preprocessor::UNKNOWN_GCC_DIAGNOSTIC_PRAGMA` | preprocessor | preprocessor | `unknown-gcc-diagnostic-pragma` | a GCC diagnostic pragma has an unknown action |
| `CCC1329` | `preprocessor::UNKNOWN_PRAGMA` | preprocessor | preprocessor | `unknown-pragma` | an unknown pragma was evaluated |
| `CCC1331` | `preprocessor::INVALID_LINEMARKER_NUMBER` | preprocessor | preprocessor | `invalid-linemarker-number` | a GNU linemarker has an invalid line number |
| `CCC1332` | `preprocessor::INVALID_LINEMARKER_FLAG` | preprocessor | preprocessor | `invalid-linemarker-flag` | a GNU linemarker has an invalid flag |
| `CCC1333` | `preprocessor::MAIN_FILE_SYSTEM_HEADER_PRAGMA` | preprocessor | preprocessor | `main-file-system-header-pragma` | #pragma GCC system\_header appears in the main file |
| `CCC1334` | `preprocessor::HEADER_READ_FAILURE` | preprocessor | preprocessor | `header-read-failure` | an included header cannot be read |
| `CCC1335` | `preprocessor::INCLUDE_CYCLE_DEPTH_LIMIT` | preprocessor | preprocessor | `include-cycle-depth-limit` | an include cycle reached the configured depth limit |
| `CCC1399` | `preprocessor::TOO_MANY_DIAGNOSTICS` | preprocessor | preprocessor | `too-many-diagnostics` | preprocessing reached its configured diagnostic limit |
| `CCC2274` | `semantic::UNDECLARED_IDENTIFIER` | semantic analysis | semantic analysis | `undeclared-identifier` | an expression uses an undeclared identifier |
<!-- END GENERATED DIAGNOSTIC REGISTRY -->

## Validation and compatibility

The registry is the only constructor for typed diagnostic codes. Its
compile-time checks enforce the `CCCdddd` format, unique codes and metadata,
non-overlapping owner bands, owner-range membership, nonempty documentation
keys, stable numeric ordering, and a one-to-one relationship between typed
constants and registry definitions. Every owner is also required to be an
allowed emitter for its code. A unit test generates the Markdown table above
from `ALL` and checks the checked-in copy byte for byte.

Public diagnostic structures still store `String` or `&'static str` values.
Typed constants convert at those existing boundaries, preserving all text and
JSON serialization. Exact string assertions remain useful compatibility tests
throughout the incremental migration.
