//! Token conversion scaffolding for phase 7 and a future parser.

use ccc_pp::{PpToken, PpTokenKind};
use ccc_session::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Keyword,
    Identifier,
    IntegerConstant,
    StringLiteral,
    CharacterConstant,
    Punctuator,
}

impl TokenKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Identifier => "identifier",
            Self::IntegerConstant => "integer-constant",
            Self::StringLiteral => "string-literal",
            Self::CharacterConstant => "character-constant",
            Self::Punctuator => "punctuator",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub spelling: String,
    pub span: Span,
}

/// Converts the currently supported pp-token forms to parser-token categories.
///
/// Numeric decoding and adjacent string concatenation are intentionally left to
/// the full phase-7 implementation; this conversion exists so `--dump-tokens`
/// exposes parser tokens rather than preprocessing-token internals.
pub fn convert_pp_tokens(tokens: Vec<PpToken>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| Token {
            kind: token_kind(&token),
            spelling: token.spelling,
            span: token.span,
        })
        .collect()
}

fn token_kind(token: &PpToken) -> TokenKind {
    match token.kind {
        PpTokenKind::Identifier if is_keyword(&token.spelling) => TokenKind::Keyword,
        PpTokenKind::Identifier => TokenKind::Identifier,
        PpTokenKind::PpNumber => TokenKind::IntegerConstant,
        PpTokenKind::StringLiteral => TokenKind::StringLiteral,
        PpTokenKind::CharacterConstant => TokenKind::CharacterConstant,
        PpTokenKind::Punctuator => TokenKind::Punctuator,
    }
}

fn is_keyword(spelling: &str) -> bool {
    matches!(
        spelling,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

#[cfg(test)]
mod tests {
    use ccc_pp::lex;
    use ccc_session::SourceMap;

    use super::*;

    #[test]
    fn distinguishes_keywords_from_identifiers() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int integer;");
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());

        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
    }
}
