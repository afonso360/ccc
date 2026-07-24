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

macro_rules! define_diagnostic_codes {
    (
        $(
            $module:ident {
                $(
                    $name:ident => {
                        code: $code:literal,
                        owner: $owner:ident,
                        emitters: [$($emitter:ident),+ $(,)?],
                        summary: $summary:literal,
                        docs: $docs_key:literal $(,)?
                    };
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

        /// Registered diagnostic codes in ascending numeric order.
        ///
        /// This is intentionally a partial registry while existing raw code
        /// sites are migrated incrementally.
        pub const ALL: &[DiagnosticCodeDefinition] = &[
            $(
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
            )+
        ];

        const _: () = validate_registry(ALL);
    };
}

define_diagnostic_codes! {
    diagnostics {
        TOO_MANY_ERRORS => {
            code: "CCC0000",
            owner: Diagnostics,
            emitters: [Diagnostics],
            summary: "the compilation reached its configured error limit",
            docs: "too-many-errors",
        };
    }
    preprocessor {
        UNTERMINATED_LITERAL => {
            code: "CCC0002",
            owner: Preprocessor,
            emitters: [Preprocessor],
            summary: "a character or string literal is unterminated",
            docs: "unterminated-literal",
        };
        WARNING_DIRECTIVE => {
            code: "CCC1315",
            owner: Preprocessor,
            emitters: [Preprocessor],
            summary: "a #warning preprocessing directive was evaluated",
            docs: "warning-directive",
        };
    }
    semantic {
        UNDECLARED_IDENTIFIER => {
            code: "CCC2274",
            owner: Semantic,
            emitters: [Semantic],
            summary: "an expression uses an undeclared identifier",
            docs: "undeclared-identifier",
        };
    }
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
    use super::*;

    #[test]
    fn registry_has_stable_numeric_order_and_metadata() {
        let entries = ALL
            .iter()
            .map(|definition| {
                (
                    definition.code.as_str(),
                    definition.symbolic_name,
                    definition.owner,
                    definition.allowed_emitters,
                    definition.docs_key,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            vec![
                (
                    "CCC0000",
                    "diagnostics::TOO_MANY_ERRORS",
                    DiagnosticOwner::Diagnostics,
                    &[DiagnosticOwner::Diagnostics][..],
                    "too-many-errors",
                ),
                (
                    "CCC0002",
                    "preprocessor::UNTERMINATED_LITERAL",
                    DiagnosticOwner::Preprocessor,
                    &[DiagnosticOwner::Preprocessor][..],
                    "unterminated-literal",
                ),
                (
                    "CCC1315",
                    "preprocessor::WARNING_DIRECTIVE",
                    DiagnosticOwner::Preprocessor,
                    &[DiagnosticOwner::Preprocessor][..],
                    "warning-directive",
                ),
                (
                    "CCC2274",
                    "semantic::UNDECLARED_IDENTIFIER",
                    DiagnosticOwner::Semantic,
                    &[DiagnosticOwner::Semantic][..],
                    "undeclared-identifier",
                ),
            ]
        );
    }

    #[test]
    fn typed_codes_preserve_display_and_diagnostic_storage() {
        let code = codes::semantic::UNDECLARED_IDENTIFIER;
        assert_eq!(code.as_str(), "CCC2274");
        assert_eq!(code.to_string(), "CCC2274");

        let diagnostic = crate::Diagnostic::error(code, "unknown name");
        assert_eq!(diagnostic.code, "CCC2274");
    }
}
