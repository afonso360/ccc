use std::collections::BTreeSet;

use ccc_session::Span;

/// The lexical class of a C preprocessing token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PpTokenKind {
    Identifier,
    PpNumber,
    StringLiteral,
    CharacterConstant,
    Punctuator,
}

impl PpTokenKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier",
            Self::PpNumber => "pp-number",
            Self::StringLiteral => "string-literal",
            Self::CharacterConstant => "character-constant",
            Self::Punctuator => "punctuator",
        }
    }
}

/// A preprocessing token with the spacing state needed for faithful rescanning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpToken {
    pub kind: PpTokenKind,
    pub span: Span,
    pub spelling: String,
    /// Whether whitespace preceded this token on its logical source line.
    pub leading_space: bool,
    /// Whether this is the first token on a logical source line.
    pub at_start_of_line: bool,
    /// Logical line at token formation time. Lines are one-based.
    pub logical_line: usize,
    /// True when the token came from a system-header region.
    pub is_system_header: bool,
    /// Macro names that must not be expanded when this token is rescanned.
    ///
    /// This is internal preprocessing state rather than source provenance.  It
    /// is retained on substituted and pasted tokens so recursion suppression
    /// survives argument prescanning and replacement-list rescanning.
    pub(crate) hide_set: BTreeSet<String>,
    /// Number of nested replacement rescans that produced this token.
    pub(crate) expansion_depth: usize,
}

impl PpToken {
    pub(crate) fn direct(
        kind: PpTokenKind,
        span: Span,
        spelling: String,
        leading_space: bool,
        at_start_of_line: bool,
        logical_line: usize,
    ) -> Self {
        Self {
            kind,
            span,
            spelling,
            leading_space,
            at_start_of_line,
            logical_line,
            is_system_header: false,
            hide_set: BTreeSet::new(),
            expansion_depth: 0,
        }
    }

    pub(crate) fn synthetic(kind: PpTokenKind, span: Span, spelling: impl Into<String>) -> Self {
        Self {
            kind,
            span,
            spelling: spelling.into(),
            leading_space: false,
            at_start_of_line: false,
            logical_line: 1,
            is_system_header: false,
            hide_set: BTreeSet::new(),
            expansion_depth: 0,
        }
    }

    pub(crate) fn identifier_key(&self) -> String {
        canonicalize_identifier(&self.spelling).unwrap_or_else(|| self.spelling.clone())
    }
}

/// Canonicalizes universal-character-name spellings in an identifier.
pub fn canonicalize_identifier(spelling: &str) -> Option<String> {
    let mut output = String::with_capacity(spelling.len());
    let mut chars = spelling.char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }

        let (_, marker) = chars.next()?;
        let count = match marker {
            'u' => 4,
            'U' => 8,
            _ => return None,
        };
        let mut value = 0_u32;
        for _ in 0..count {
            let (_, digit) = chars.next()?;
            value = value.checked_mul(16)?.checked_add(digit.to_digit(16)?)?;
        }
        let decoded = char::from_u32(value)?;
        if is_disallowed_ucn(decoded) {
            return None;
        }
        output.push(decoded);
    }
    Some(output)
}

pub(crate) fn is_disallowed_ucn(character: char) -> bool {
    let value = character as u32;
    value < 0xA0 && !matches!(character, '$' | '@' | '`')
}
