//! Preprocessing-item conversion into decoded parser tokens.

use std::fmt;

use ccc_pp::{
    CharacterConstant, FloatingConstant, IntegerConstant, LineMarker, PpItem, PpToken, PpTokenKind,
    PragmaEvent, StringLiteral, concatenate_string_literals, decode_character_constant,
    decode_floating_constant, decode_integer_constant, decode_string_literal,
};
use ccc_session::Span;

use crate::{Keyword, Punctuator};

use super::span_through;

/// A phase-7 parser token with literal spellings decoded exactly once.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub spelling: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Keyword(Keyword),
    Identifier,
    Integer(IntegerConstant),
    Floating(FloatingConstant),
    Character(CharacterConstant),
    String(StringLiteral),
    Punctuator(Punctuator),
}

/// Ordered phase-7 input. Pragmas remain between the tokens around them.
#[derive(Clone, Debug, PartialEq)]
pub enum FrontendItem {
    Token(Token),
    Pragma(PragmaEvent),
    LineMarker(LineMarker),
    Newline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenConversionError {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
}

impl fmt::Display for TokenConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for TokenConversionError {}

/// Converts the canonical preprocessing item stream without discarding pragma
/// or location events. Adjacent string tokens are decoded and concatenated as
/// translation phase 6 requires; intervening non-token events keep their order.
pub fn convert_pp_items(
    items: impl IntoIterator<Item = PpItem>,
) -> Result<Vec<FrontendItem>, TokenConversionError> {
    struct PendingStrings {
        values: Vec<StringLiteral>,
        spellings: Vec<String>,
        first_span: Span,
        last_span: Span,
        events: Vec<FrontendItem>,
    }

    fn flush(
        output: &mut Vec<FrontendItem>,
        pending: &mut Option<PendingStrings>,
    ) -> Result<(), TokenConversionError> {
        let Some(pending) = pending.take() else {
            return Ok(());
        };
        let literal =
            concatenate_string_literals(&pending.values).map_err(|error| TokenConversionError {
                code: "CCC1012",
                span: pending.first_span,
                message: error.message,
            })?;
        let span = span_through(pending.first_span, pending.last_span);
        output.push(FrontendItem::Token(Token {
            kind: TokenKind::String(literal),
            spelling: pending.spellings.join(" "),
            span,
        }));
        output.extend(pending.events);
        Ok(())
    }

    let mut output = Vec::new();
    let mut pending = None::<PendingStrings>;
    for item in items {
        match item {
            PpItem::Token(token) if token.kind == PpTokenKind::StringLiteral => {
                let value = decode_string_literal(&token.spelling).map_err(|error| {
                    TokenConversionError {
                        code: "CCC1011",
                        span: token.span,
                        message: error.message,
                    }
                })?;
                if let Some(pending) = &mut pending {
                    pending.values.push(value);
                    pending.spellings.push(token.spelling);
                    pending.last_span = token.span;
                } else {
                    pending = Some(PendingStrings {
                        values: vec![value],
                        spellings: vec![token.spelling],
                        first_span: token.span,
                        last_span: token.span,
                        events: Vec::new(),
                    });
                }
            }
            PpItem::Token(token) => {
                flush(&mut output, &mut pending)?;
                output.push(FrontendItem::Token(convert_token(token)?));
            }
            item => {
                let event = match item {
                    PpItem::Pragma(pragma) => FrontendItem::Pragma(pragma),
                    PpItem::LineMarker(marker) => FrontendItem::LineMarker(marker),
                    PpItem::Newline => FrontendItem::Newline,
                    PpItem::Token(_) => unreachable!("token cases were handled above"),
                };
                if let Some(pending) = &mut pending {
                    pending.events.push(event);
                } else {
                    output.push(event);
                }
            }
        }
    }
    flush(&mut output, &mut pending)?;
    Ok(output)
}

fn convert_token(token: PpToken) -> Result<Token, TokenConversionError> {
    let kind = match token.kind {
        PpTokenKind::Identifier => Keyword::from_spelling(&token.spelling)
            .map_or(TokenKind::Identifier, TokenKind::Keyword),
        PpTokenKind::PpNumber => match decode_integer_constant(&token.spelling) {
            Ok(value) => TokenKind::Integer(value),
            Err(integer_error) => decode_floating_constant(&token.spelling)
                .map(TokenKind::Floating)
                .map_err(|floating_error| TokenConversionError {
                    code: "CCC1010",
                    span: token.span,
                    message: format!(
                        "invalid numeric constant `{}`: {}; {}",
                        token.spelling, integer_error.message, floating_error.message
                    ),
                })?,
        },
        PpTokenKind::CharacterConstant => decode_character_constant(&token.spelling)
            .map(TokenKind::Character)
            .map_err(|error| TokenConversionError {
                code: "CCC1013",
                span: token.span,
                message: error.message,
            })?,
        PpTokenKind::StringLiteral => unreachable!("strings are combined by convert_pp_items"),
        PpTokenKind::Punctuator => {
            TokenKind::Punctuator(Punctuator::from_spelling(&token.spelling))
        }
    };
    Ok(Token {
        kind,
        spelling: token.spelling,
        span: token.span,
    })
}
