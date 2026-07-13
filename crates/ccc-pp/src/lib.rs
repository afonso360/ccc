//! Preprocessing-token formation for the first compiler slice.

use std::fmt;

use ccc_session::{FileId, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpToken {
    pub kind: PpTokenKind,
    pub span: Span,
    pub spelling: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexError {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}", self.message, self.span)
    }
}

impl std::error::Error for LexError {}

/// Lexes C preprocessing tokens, omitting whitespace and comments.
pub fn lex(file: FileId, source: &str) -> Result<Vec<PpToken>, LexError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }

        if source[index..].starts_with("//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }

        if source[index..].starts_with("/*") {
            let start = index;
            index += 2;
            while index + 1 < bytes.len() && &bytes[index..index + 2] != b"*/" {
                index += 1;
            }
            if index + 1 >= bytes.len() {
                return Err(error(
                    "CCC0001",
                    file,
                    start,
                    bytes.len(),
                    "unterminated block comment",
                ));
            }
            index += 2;
            continue;
        }

        let start = index;
        let kind = if is_identifier_start(bytes[index]) {
            index += 1;
            while index < bytes.len() && is_identifier_continue(bytes[index]) {
                index += 1;
            }
            PpTokenKind::Identifier
        } else if bytes[index].is_ascii_digit()
            || (bytes[index] == b'.'
                && bytes
                    .get(index + 1)
                    .is_some_and(|byte| byte.is_ascii_digit()))
        {
            index += 1;
            while index < bytes.len() && is_pp_number_continue(bytes[index], bytes[index - 1]) {
                index += 1;
            }
            PpTokenKind::PpNumber
        } else if matches!(bytes[index], b'\'' | b'\"') {
            let quote = bytes[index];
            index += 1;
            let mut terminated = false;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' if index + 1 < bytes.len() => index += 2,
                    character if character == quote => {
                        index += 1;
                        terminated = true;
                        break;
                    }
                    _ => index += 1,
                }
            }
            if !terminated {
                return Err(error(
                    "CCC0002",
                    file,
                    start,
                    bytes.len(),
                    "unterminated literal",
                ));
            }
            if quote == b'\'' {
                PpTokenKind::CharacterConstant
            } else {
                PpTokenKind::StringLiteral
            }
        } else if let Some(length) = punctuator_length(&source[index..]) {
            index += length;
            PpTokenKind::Punctuator
        } else {
            let character_length = source[index..]
                .chars()
                .next()
                .expect("index is in bounds")
                .len_utf8();
            return Err(error(
                "CCC0003",
                file,
                start,
                start + character_length,
                "invalid preprocessing character",
            ));
        };

        tokens.push(PpToken {
            kind,
            span: Span::new(file, start, index),
            spelling: source[start..index].to_owned(),
        });
    }

    Ok(tokens)
}

fn error(code: &'static str, file: FileId, start: usize, end: usize, message: &str) -> LexError {
    LexError {
        code,
        span: Span::new(file, start, end),
        message: message.to_owned(),
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn is_pp_number_continue(byte: u8, previous: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'.' | b'_')
        || (matches!(byte, b'+' | b'-') && matches!(previous, b'e' | b'E' | b'p' | b'P'))
}

fn punctuator_length(rest: &str) -> Option<usize> {
    const MULTI_CHARACTER: &[&str] = &[
        "%:%:", "<<=", ">>=", "...", "->", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&",
        "||", "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "##", "<:", ":>", "<%", "%>", "%:",
    ];
    const SINGLE_CHARACTER: &str = "[](){}.,;:?~!+-*/%&|^<>=#";

    MULTI_CHARACTER
        .iter()
        .find(|punctuator| rest.starts_with(**punctuator))
        .map(|punctuator| punctuator.len())
        .or_else(|| {
            rest.chars()
                .next()
                .filter(|character| SINGLE_CHARACTER.contains(*character))
                .map(char::len_utf8)
        })
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

    use super::*;

    #[test]
    fn forms_tokens_and_drops_comments() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int /* ignored */ main(void) { return 42; }");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();

        assert_eq!(
            spellings,
            [
                "int", "main", "(", "void", ")", "{", "return", "42", ";", "}"
            ]
        );
    }

    #[test]
    fn rejects_an_unterminated_block_comment() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "/*");

        let error = lex(file, sources.source(file).unwrap()).unwrap_err();
        assert_eq!(error.code, "CCC0001");
        assert_eq!(error.message, "unterminated block comment");
    }

    #[test]
    fn keeps_arithmetic_signs_outside_pp_numbers() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "1+2 1-2 1e+2 0x1p-2");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();

        assert_eq!(spellings, ["1", "+", "2", "1", "-", "2", "1e+2", "0x1p-2"]);
    }

    #[test]
    fn classifies_an_invalid_preprocessing_character() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "\\");
        let error = lex(file, sources.source(file).unwrap()).unwrap_err();
        assert_eq!(error.code, "CCC0003");
    }
}
