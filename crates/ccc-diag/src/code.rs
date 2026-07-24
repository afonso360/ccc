use std::fmt;

/// A stable diagnostic identifier.
///
/// Values can only be created by the registry in this module. Compiler
/// components should use the named constants in [`codes`] instead of spelling
/// diagnostic identifiers at emission or control-flow sites.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    const fn new(code: &'static str) -> Self {
        Self(code)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<DiagnosticCode> for String {
    fn from(code: DiagnosticCode) -> Self {
        code.as_str().to_owned()
    }
}

/// The component responsible for allocating and documenting a diagnostic code.
///
/// Ownership is distinct from propagation and observation: another component
/// may transport or inspect a code without owning its number or semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum DiagnosticOwner {
    Diagnostics,
    Preprocessor,
    Syntax,
    Semantic,
    Ir,
    Abi,
    Codegen,
    Link,
    Driver,
}

impl DiagnosticOwner {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Diagnostics => "diagnostics",
            Self::Preprocessor => "preprocessor",
            Self::Syntax => "syntax",
            Self::Semantic => "semantic analysis",
            Self::Ir => "IR",
            Self::Abi => "ABI",
            Self::Codegen => "code generation",
            Self::Link => "linking",
            Self::Driver => "driver",
        }
    }
}

/// An inclusive numeric range allocated to a diagnostic owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticOwnerBand {
    pub owner: DiagnosticOwner,
    pub start: u16,
    pub end: u16,
}

/// Allocated diagnostic-code ranges.
///
/// The preprocessor owns several disjoint historical ranges. Gaps outside
/// these bands remain reserved rather than being assigned implicitly.
pub const OWNER_BANDS: &[DiagnosticOwnerBand] = &[
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Diagnostics,
        start: 0,
        end: 0,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Preprocessor,
        start: 1,
        end: 9,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Preprocessor,
        start: 1000,
        end: 1009,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Syntax,
        start: 1010,
        end: 1099,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Preprocessor,
        start: 1100,
        end: 1399,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Semantic,
        start: 2200,
        end: 2499,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Ir,
        start: 3100,
        end: 3199,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Abi,
        start: 3500,
        end: 3599,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Codegen,
        start: 4000,
        end: 4099,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Link,
        start: 5000,
        end: 5099,
    },
    DiagnosticOwnerBand {
        owner: DiagnosticOwner::Driver,
        start: 6000,
        end: 6099,
    },
];

/// Registry metadata for one stable diagnostic code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticCodeDefinition {
    pub code: DiagnosticCode,
    pub symbolic_name: &'static str,
    pub owner: DiagnosticOwner,
    pub allowed_emitters: &'static [DiagnosticOwner],
    pub summary: &'static str,
    pub docs_key: &'static str,
}

macro_rules! define_diagnostic_code_constants {
    (
        $(
            $module:ident {
                $(
                    $name:ident => $code:literal;
                )+
            }
        )+
    ) => {
        pub mod codes {
            $(
                pub mod $module {
                    use super::super::DiagnosticCode;

                    $(
                        pub const $name: DiagnosticCode = DiagnosticCode::new($code);
                    )+
                }
            )+
        }

        const DIAGNOSTIC_CODE_CONSTANT_COUNT: usize =
            0 $( $(+ { let _ = stringify!($name); 1 })+ )+;
    };
}

macro_rules! define_diagnostic_registry {
    (
        $(
            $module:ident::$name:ident => {
                owner: $owner:ident,
                emitters: [$($emitter:ident),+ $(,)?],
                summary: $summary:literal,
                docs: $docs_key:literal $(,)?
            };
        )+
    ) => {
        /// Registered diagnostic codes in ascending numeric order.
        ///
        /// This is intentionally a partial registry while existing raw code
        /// sites are migrated incrementally.
        pub const ALL: &[DiagnosticCodeDefinition] = &[
            $(
                DiagnosticCodeDefinition {
                    code: codes::$module::$name,
                    symbolic_name: concat!(stringify!($module), "::", stringify!($name)),
                    owner: DiagnosticOwner::$owner,
                    allowed_emitters: &[$(DiagnosticOwner::$emitter),+],
                    summary: $summary,
                    docs_key: $docs_key,
                },
            )+
        ];

        const _: () = {
            assert!(
                ALL.len() == DIAGNOSTIC_CODE_CONSTANT_COUNT,
                "every typed diagnostic code must have exactly one registry definition"
            );
            validate_registry(ALL);
        };
    };
}

define_diagnostic_code_constants! {
    diagnostics {
        TOO_MANY_ERRORS => "CCC0000";
    }
    preprocessor {
        UNTERMINATED_BLOCK_COMMENT => "CCC0001";
        UNTERMINATED_LITERAL => "CCC0002";
        INVALID_IDENTIFIER_UCN => "CCC0004";
        UTF8_CHARACTER_CONSTANT => "CCC0005";
        INVALID_CHARACTER_UCN => "CCC0006";
        TRIGRAPH => "CCC1001";
        MACRO_EXPANSION_DEPTH_LIMIT => "CCC1101";
        PREPROCESSING_TOKEN_LIMIT => "CCC1102";
        MACRO_ARGUMENT_DEPTH_LIMIT => "CCC1103";
        MACRO_ARGUMENT_COUNT => "CCC1104";
        UNTERMINATED_MACRO_INVOCATION => "CCC1105";
        INVALID_STRINGIZE_OPERAND => "CCC1106";
        INVALID_TOKEN_PASTE_POSITION => "CCC1107";
        INVALID_TOKEN_PASTE_RESULT => "CCC1108";
        INVALID_PRAGMA_OPERATOR => "CCC1109";
        MACRO_REDEFINED => "CCC1110";
        UNSUPPORTED_VA_OPT => "CCC1111";
        VA_ARGS_OUTSIDE_VARIADIC_MACRO => "CCC1112";
        UNEXPECTED_CONDITIONAL_TOKEN => "CCC1201";
        DEFINED_REQUIRES_IDENTIFIER => "CCC1202";
        DEFINED_MISSING_CLOSE => "CCC1203";
        PREDICATE_REQUIRES_PARENTHESES => "CCC1204";
        UNTERMINATED_PREDICATE => "CCC1205";
        CONDITIONAL_MISSING_COLON => "CCC1206";
        SHIFT_COUNT_TOO_LARGE => "CCC1207";
        CONDITIONAL_DIVISION_BY_ZERO => "CCC1208";
        CONDITIONAL_MISSING_CLOSE => "CCC1209";
        CONDITIONAL_EXPECTED_EXPRESSION => "CCC1210";
        INVALID_CONDITIONAL_INTEGER => "CCC1211";
        INVALID_CONDITIONAL_CHARACTER => "CCC1212";
        INVALID_CONDITIONAL_TOKEN => "CCC1213";
        INVALID_COMMAND_LINE_MACRO_NAME => "CCC1301";
        FORCED_INPUT_UNAVAILABLE => "CCC1302";
        INCLUDE_DEPTH_LIMIT => "CCC1303";
        UNKNOWN_SOURCE_OCCURRENCE => "CCC1304";
        UNTERMINATED_CONDITIONAL_DIRECTIVE => "CCC1305";
        CONDITIONAL_REQUIRES_IDENTIFIER => "CCC1306";
        ELIF_WITHOUT_IF => "CCC1307";
        ELIF_AFTER_ELSE => "CCC1308";
        ELSE_WITHOUT_IF => "CCC1309";
        TOKENS_AFTER_ELSE => "CCC1310";
        DUPLICATE_ELSE => "CCC1311";
        TOKENS_AFTER_ENDIF => "CCC1312";
        ENDIF_WITHOUT_IF => "CCC1313";
        ERROR_DIRECTIVE => "CCC1314";
        WARNING_DIRECTIVE => "CCC1315";
        UNKNOWN_DIRECTIVE => "CCC1316";
        DEFINE_REQUIRES_NAME => "CCC1317";
        INVALID_VARIADIC_PARAMETER_POSITION => "CCC1318";
        EXPECTED_MACRO_PARAMETER => "CCC1319";
        DUPLICATE_MACRO_PARAMETER => "CCC1320";
        INVALID_MACRO_PARAMETER_LIST => "CCC1321";
        INVALID_UNDEF => "CCC1322";
        INVALID_INCLUDE_OPERAND => "CCC1323";
        HEADER_NOT_FOUND => "CCC1324";
        INVALID_LINE_NUMBER => "CCC1325";
        INVALID_LINE_FILE_NAME => "CCC1326";
        TOKENS_AFTER_LINE => "CCC1327";
        UNKNOWN_GCC_DIAGNOSTIC_PRAGMA => "CCC1328";
        UNKNOWN_PRAGMA => "CCC1329";
        INVALID_LINEMARKER_NUMBER => "CCC1331";
        INVALID_LINEMARKER_FLAG => "CCC1332";
        MAIN_FILE_SYSTEM_HEADER_PRAGMA => "CCC1333";
        HEADER_READ_FAILURE => "CCC1334";
        INCLUDE_CYCLE_DEPTH_LIMIT => "CCC1335";
        TOO_MANY_DIAGNOSTICS => "CCC1399";
    }
    syntax {
        INVALID_NUMERIC_CONSTANT => "CCC1010";
        INVALID_STRING_LITERAL => "CCC1011";
        INCOMPATIBLE_STRING_LITERAL_CONCATENATION => "CCC1012";
        INVALID_CHARACTER_CONSTANT => "CCC1013";
        PARSE_ERROR => "CCC1020";
    }
    semantic {
        UNDECLARED_IDENTIFIER => "CCC2274";
    }
}

define_diagnostic_registry! {
    diagnostics::TOO_MANY_ERRORS => {
        owner: Diagnostics,
        emitters: [Diagnostics],
        summary: "the compilation reached its configured error limit",
        docs: "too-many-errors",
    };
    preprocessor::UNTERMINATED_BLOCK_COMMENT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a block comment is unterminated",
        docs: "unterminated-block-comment",
    };
    preprocessor::UNTERMINATED_LITERAL => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a character or string literal is unterminated",
        docs: "unterminated-literal",
    };
    preprocessor::INVALID_IDENTIFIER_UCN => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an identifier contains an invalid universal character name",
        docs: "invalid-identifier-ucn",
    };
    preprocessor::UTF8_CHARACTER_CONSTANT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a character constant uses the invalid u8 prefix",
        docs: "utf8-character-constant",
    };
    preprocessor::INVALID_CHARACTER_UCN => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a character constant contains an invalid universal character name",
        docs: "invalid-character-ucn",
    };
    preprocessor::TRIGRAPH => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a trigraph was converted or ignored",
        docs: "trigraph",
    };
    syntax::INVALID_NUMERIC_CONSTANT => {
        owner: Syntax,
        emitters: [Syntax],
        summary: "a preprocessing number is not a valid numeric constant",
        docs: "invalid-numeric-constant",
    };
    syntax::INVALID_STRING_LITERAL => {
        owner: Syntax,
        emitters: [Syntax],
        summary: "a string literal cannot be decoded",
        docs: "invalid-string-literal",
    };
    syntax::INCOMPATIBLE_STRING_LITERAL_CONCATENATION => {
        owner: Syntax,
        emitters: [Syntax],
        summary: "adjacent string literals cannot be concatenated",
        docs: "incompatible-string-literal-concatenation",
    };
    syntax::INVALID_CHARACTER_CONSTANT => {
        owner: Syntax,
        emitters: [Syntax],
        summary: "a character constant cannot be decoded",
        docs: "invalid-character-constant",
    };
    syntax::PARSE_ERROR => {
        owner: Syntax,
        emitters: [Syntax],
        summary: "the token stream does not satisfy C syntax",
        docs: "parse-error",
    };
    preprocessor::MACRO_EXPANSION_DEPTH_LIMIT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "macro expansion exceeded its configured depth limit",
        docs: "macro-expansion-depth-limit",
    };
    preprocessor::PREPROCESSING_TOKEN_LIMIT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "macro expansion exceeded its configured token limit",
        docs: "preprocessing-token-limit",
    };
    preprocessor::MACRO_ARGUMENT_DEPTH_LIMIT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "macro arguments exceeded their configured nesting limit",
        docs: "macro-argument-depth-limit",
    };
    preprocessor::MACRO_ARGUMENT_COUNT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a macro invocation has the wrong number of arguments",
        docs: "macro-argument-count",
    };
    preprocessor::UNTERMINATED_MACRO_INVOCATION => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a function-like macro invocation is unterminated",
        docs: "unterminated-macro-invocation",
    };
    preprocessor::INVALID_STRINGIZE_OPERAND => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "the stringize operator does not precede a macro parameter",
        docs: "invalid-stringize-operand",
    };
    preprocessor::INVALID_TOKEN_PASTE_POSITION => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "the token-paste operator appears without both operands",
        docs: "invalid-token-paste-position",
    };
    preprocessor::INVALID_TOKEN_PASTE_RESULT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "token pasting does not form one preprocessing token",
        docs: "invalid-token-paste-result",
    };
    preprocessor::INVALID_PRAGMA_OPERATOR => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a _Pragma operator has an invalid operand",
        docs: "invalid-pragma-operator",
    };
    preprocessor::MACRO_REDEFINED => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a macro is redefined with a different replacement",
        docs: "macro-redefined",
    };
    preprocessor::UNSUPPORTED_VA_OPT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "__VA_OPT__ is unavailable in the selected compatibility profile",
        docs: "unsupported-va-opt",
    };
    preprocessor::VA_ARGS_OUTSIDE_VARIADIC_MACRO => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "__VA_ARGS__ appears outside a variadic macro replacement",
        docs: "va-args-outside-variadic-macro",
    };
    preprocessor::UNEXPECTED_CONDITIONAL_TOKEN => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression has an unexpected trailing token",
        docs: "unexpected-conditional-token",
    };
    preprocessor::DEFINED_REQUIRES_IDENTIFIER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "the defined operator has no identifier operand",
        docs: "defined-requires-identifier",
    };
    preprocessor::DEFINED_MISSING_CLOSE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "the parenthesized defined operator is not closed",
        docs: "defined-missing-close",
    };
    preprocessor::PREDICATE_REQUIRES_PARENTHESES => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional feature predicate is not parenthesized",
        docs: "predicate-requires-parentheses",
    };
    preprocessor::UNTERMINATED_PREDICATE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional feature predicate is unterminated",
        docs: "unterminated-predicate",
    };
    preprocessor::CONDITIONAL_MISSING_COLON => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression is missing its colon",
        docs: "conditional-missing-colon",
    };
    preprocessor::SHIFT_COUNT_TOO_LARGE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional-expression shift count is too large",
        docs: "shift-count-too-large",
    };
    preprocessor::CONDITIONAL_DIVISION_BY_ZERO => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression divides by zero",
        docs: "conditional-division-by-zero",
    };
    preprocessor::CONDITIONAL_MISSING_CLOSE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a parenthesized conditional expression is not closed",
        docs: "conditional-missing-close",
    };
    preprocessor::CONDITIONAL_EXPECTED_EXPRESSION => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression is missing an operand",
        docs: "conditional-expected-expression",
    };
    preprocessor::INVALID_CONDITIONAL_INTEGER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression contains an invalid integer constant",
        docs: "invalid-conditional-integer",
    };
    preprocessor::INVALID_CONDITIONAL_CHARACTER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression contains an invalid character constant",
        docs: "invalid-conditional-character",
    };
    preprocessor::INVALID_CONDITIONAL_TOKEN => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional expression contains an invalid token",
        docs: "invalid-conditional-token",
    };
    preprocessor::INVALID_COMMAND_LINE_MACRO_NAME => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a command-line macro has an invalid name",
        docs: "invalid-command-line-macro-name",
    };
    preprocessor::FORCED_INPUT_UNAVAILABLE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a forced preprocessing input cannot be opened or read",
        docs: "forced-input-unavailable",
    };
    preprocessor::INCLUDE_DEPTH_LIMIT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "header inclusion exceeded its configured depth limit",
        docs: "include-depth-limit",
    };
    preprocessor::UNKNOWN_SOURCE_OCCURRENCE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "the preprocessor cannot resolve a source occurrence",
        docs: "unknown-source-occurrence",
    };
    preprocessor::UNTERMINATED_CONDITIONAL_DIRECTIVE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional directive group is unterminated",
        docs: "unterminated-conditional-directive",
    };
    preprocessor::CONDITIONAL_REQUIRES_IDENTIFIER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #ifdef or #ifndef directive does not name one identifier",
        docs: "conditional-requires-identifier",
    };
    preprocessor::ELIF_WITHOUT_IF => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #elif directive has no matching #if",
        docs: "elif-without-if",
    };
    preprocessor::ELIF_AFTER_ELSE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #elif directive follows #else in the same group",
        docs: "elif-after-else",
    };
    preprocessor::ELSE_WITHOUT_IF => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #else directive has no matching #if",
        docs: "else-without-if",
    };
    preprocessor::TOKENS_AFTER_ELSE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "tokens follow an #else directive",
        docs: "tokens-after-else",
    };
    preprocessor::DUPLICATE_ELSE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a conditional group contains more than one #else",
        docs: "duplicate-else",
    };
    preprocessor::TOKENS_AFTER_ENDIF => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "tokens follow an #endif directive",
        docs: "tokens-after-endif",
    };
    preprocessor::ENDIF_WITHOUT_IF => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #endif directive has no matching #if",
        docs: "endif-without-if",
    };
    preprocessor::ERROR_DIRECTIVE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #error preprocessing directive was evaluated",
        docs: "error-directive",
    };
    preprocessor::WARNING_DIRECTIVE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a #warning preprocessing directive was evaluated",
        docs: "warning-directive",
    };
    preprocessor::UNKNOWN_DIRECTIVE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an unknown preprocessing directive was evaluated",
        docs: "unknown-directive",
    };
    preprocessor::DEFINE_REQUIRES_NAME => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a #define directive does not name a macro",
        docs: "define-requires-name",
    };
    preprocessor::INVALID_VARIADIC_PARAMETER_POSITION => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a variadic macro parameter is not final",
        docs: "invalid-variadic-parameter-position",
    };
    preprocessor::EXPECTED_MACRO_PARAMETER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a macro parameter list does not contain an identifier",
        docs: "expected-macro-parameter",
    };
    preprocessor::DUPLICATE_MACRO_PARAMETER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a macro parameter name is duplicated",
        docs: "duplicate-macro-parameter",
    };
    preprocessor::INVALID_MACRO_PARAMETER_LIST => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a macro parameter list has invalid punctuation",
        docs: "invalid-macro-parameter-list",
    };
    preprocessor::INVALID_UNDEF => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an #undef directive does not contain exactly one identifier",
        docs: "invalid-undef",
    };
    preprocessor::INVALID_INCLUDE_OPERAND => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an include operand does not form one header name",
        docs: "invalid-include-operand",
    };
    preprocessor::HEADER_NOT_FOUND => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an included header cannot be found",
        docs: "header-not-found",
    };
    preprocessor::INVALID_LINE_NUMBER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a #line directive has an invalid line number",
        docs: "invalid-line-number",
    };
    preprocessor::INVALID_LINE_FILE_NAME => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a #line file name is not a string literal",
        docs: "invalid-line-file-name",
    };
    preprocessor::TOKENS_AFTER_LINE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "tokens follow the operands of a #line directive",
        docs: "tokens-after-line",
    };
    preprocessor::UNKNOWN_GCC_DIAGNOSTIC_PRAGMA => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a GCC diagnostic pragma has an unknown action",
        docs: "unknown-gcc-diagnostic-pragma",
    };
    preprocessor::UNKNOWN_PRAGMA => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an unknown pragma was evaluated",
        docs: "unknown-pragma",
    };
    preprocessor::INVALID_LINEMARKER_NUMBER => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a GNU linemarker has an invalid line number",
        docs: "invalid-linemarker-number",
    };
    preprocessor::INVALID_LINEMARKER_FLAG => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "a GNU linemarker has an invalid flag",
        docs: "invalid-linemarker-flag",
    };
    preprocessor::MAIN_FILE_SYSTEM_HEADER_PRAGMA => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "#pragma GCC system_header appears in the main file",
        docs: "main-file-system-header-pragma",
    };
    preprocessor::HEADER_READ_FAILURE => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an included header cannot be read",
        docs: "header-read-failure",
    };
    preprocessor::INCLUDE_CYCLE_DEPTH_LIMIT => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "an include cycle reached the configured depth limit",
        docs: "include-cycle-depth-limit",
    };
    preprocessor::TOO_MANY_DIAGNOSTICS => {
        owner: Preprocessor,
        emitters: [Preprocessor],
        summary: "preprocessing reached its configured diagnostic limit",
        docs: "too-many-diagnostics",
    };
    semantic::UNDECLARED_IDENTIFIER => {
        owner: Semantic,
        emitters: [Semantic],
        summary: "an expression uses an undeclared identifier",
        docs: "undeclared-identifier",
    };
}

const fn validate_registry(definitions: &[DiagnosticCodeDefinition]) {
    validate_owner_bands();

    let mut index = 0;
    while index < definitions.len() {
        let definition = &definitions[index];
        let number = parse_code_number(definition.code.as_str());

        assert!(
            !definition.symbolic_name.is_empty(),
            "a diagnostic symbolic name must not be empty"
        );
        assert!(
            !definition.summary.is_empty(),
            "a diagnostic summary must not be empty"
        );
        assert!(
            !definition.docs_key.is_empty(),
            "a diagnostic documentation key must not be empty"
        );
        assert!(
            owner_contains(definition.owner, number),
            "a diagnostic code is outside its owner's allocated range"
        );
        assert!(
            emitters_include_owner(definition),
            "a diagnostic owner must be an allowed emitter"
        );

        if index != 0 {
            let previous = parse_code_number(definitions[index - 1].code.as_str());
            assert!(
                previous < number,
                "the diagnostic registry must be in ascending numeric order"
            );
        }

        let mut other_index = index + 1;
        while other_index < definitions.len() {
            let other = &definitions[other_index];
            assert!(
                number != parse_code_number(other.code.as_str()),
                "duplicate diagnostic code"
            );
            assert!(
                !str_equal(definition.symbolic_name, other.symbolic_name),
                "duplicate diagnostic symbolic name"
            );
            assert!(
                !str_equal(definition.docs_key, other.docs_key),
                "duplicate diagnostic documentation key"
            );
            other_index += 1;
        }

        index += 1;
    }
}

const fn validate_owner_bands() {
    let mut index = 0;
    while index < OWNER_BANDS.len() {
        let band = OWNER_BANDS[index];
        assert!(
            band.start <= band.end,
            "a diagnostic owner band must not be inverted"
        );
        assert!(
            band.end <= 9999,
            "a diagnostic owner band must fit the CCCdddd format"
        );

        let mut other_index = index + 1;
        while other_index < OWNER_BANDS.len() {
            let other = OWNER_BANDS[other_index];
            assert!(
                band.end < other.start || other.end < band.start,
                "diagnostic owner bands must not overlap"
            );
            other_index += 1;
        }
        index += 1;
    }
}

const fn parse_code_number(code: &str) -> u16 {
    let bytes = code.as_bytes();
    assert!(
        bytes.len() == 7,
        "a diagnostic code must contain exactly seven bytes"
    );
    assert!(
        bytes[0] == b'C' && bytes[1] == b'C' && bytes[2] == b'C',
        "a diagnostic code must start with CCC"
    );

    let mut index = 3;
    let mut number = 0_u16;
    while index < bytes.len() {
        assert!(
            bytes[index].is_ascii_digit(),
            "a diagnostic suffix must contain four ASCII digits"
        );
        number = number * 10 + (bytes[index] - b'0') as u16;
        index += 1;
    }
    number
}

const fn owner_contains(owner: DiagnosticOwner, number: u16) -> bool {
    let mut index = 0;
    while index < OWNER_BANDS.len() {
        let band = OWNER_BANDS[index];
        if same_owner(owner, band.owner) && band.start <= number && number <= band.end {
            return true;
        }
        index += 1;
    }
    false
}

const fn emitters_include_owner(definition: &DiagnosticCodeDefinition) -> bool {
    let mut index = 0;
    while index < definition.allowed_emitters.len() {
        if same_owner(definition.owner, definition.allowed_emitters[index]) {
            return true;
        }
        index += 1;
    }
    false
}

const fn same_owner(left: DiagnosticOwner, right: DiagnosticOwner) -> bool {
    left as u8 == right as u8
}

const fn str_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }

    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    #[test]
    fn registry_has_stable_numeric_order_and_metadata() {
        assert_eq!(ALL.len(), 73);
        assert_eq!(
            (
                ALL[7].code.as_str(),
                ALL[7].symbolic_name,
                ALL[7].owner,
                ALL[7].allowed_emitters,
                ALL[7].docs_key,
            ),
            (
                "CCC1010",
                "syntax::INVALID_NUMERIC_CONSTANT",
                DiagnosticOwner::Syntax,
                &[DiagnosticOwner::Syntax][..],
                "invalid-numeric-constant",
            )
        );
        assert_eq!(
            (ALL[12].code.as_str(), ALL[12].symbolic_name, ALL[12].owner,),
            (
                "CCC1101",
                "preprocessor::MACRO_EXPANSION_DEPTH_LIMIT",
                DiagnosticOwner::Preprocessor,
            )
        );
        assert_eq!(
            (
                ALL.last().unwrap().code.as_str(),
                ALL.last().unwrap().symbolic_name,
            ),
            ("CCC2274", "semantic::UNDECLARED_IDENTIFIER")
        );
    }

    #[test]
    fn documentation_contains_the_generated_registry_table() {
        const START: &str = "<!-- BEGIN GENERATED DIAGNOSTIC REGISTRY -->\n";
        const END: &str = "<!-- END GENERATED DIAGNOSTIC REGISTRY -->";

        let documentation = include_str!("../../../docs/diagnostics.md");
        let (_, after_start) = documentation
            .split_once(START)
            .expect("diagnostic documentation must contain the generated-table start marker");
        let (actual, _) = after_start
            .split_once(END)
            .expect("diagnostic documentation must contain the generated-table end marker");

        assert_eq!(actual, render_registry_table());
    }

    #[test]
    fn typed_codes_preserve_display_and_diagnostic_storage() {
        let code = codes::semantic::UNDECLARED_IDENTIFIER;
        assert_eq!(code.as_str(), "CCC2274");
        assert_eq!(code.to_string(), "CCC2274");

        let diagnostic = crate::Diagnostic::error(code, "unknown name");
        assert_eq!(diagnostic.code, "CCC2274");
    }

    fn render_registry_table() -> String {
        let mut table = String::from(
            "| Code | Symbol | Owner | Allowed emitters | Documentation key | Meaning |\n\
             | --- | --- | --- | --- | --- | --- |\n",
        );
        for definition in ALL {
            let emitters = definition
                .allowed_emitters
                .iter()
                .map(|owner| owner.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                table,
                "| `{}` | `{}` | {} | {} | `{}` | {} |",
                definition.code,
                definition.symbolic_name,
                definition.owner.as_str(),
                emitters,
                definition.docs_key,
                escape_markdown_table_cell(definition.summary),
            )
            .unwrap();
        }
        table
    }

    fn escape_markdown_table_cell(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('|', "\\|")
            .replace('_', "\\_")
    }
}
