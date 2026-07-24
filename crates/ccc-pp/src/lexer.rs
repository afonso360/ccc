use std::fmt;

use ccc_diag::codes::preprocessor::UNTERMINATED_LITERAL;
use ccc_session::{FileId, Span};

use crate::literal::validate_character_constant_ucns;
use crate::normalize::{NormalizeOptions, NormalizedSource, normalize};
use crate::token::{PpToken, PpTokenKind, canonicalize_identifier};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexerOptions {
    pub trigraphs: bool,
    pub warn_trigraphs: bool,
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

#[derive(Clone, Debug)]
pub(crate) struct LexedFile {
    pub lines: Vec<Vec<PpToken>>,
    /// Recoverable lexical errors grouped by physical preprocessing line.
    /// The engine decides whether to report them after conditional inclusion
    /// state is known.
    pub line_errors: Vec<Vec<LexError>>,
    pub diagnostics: Vec<crate::diagnostic::PpDiagnostic>,
}

/// Lexes C preprocessing tokens, omitting whitespace and comments.
pub fn lex(file: FileId, source: &str) -> Result<Vec<PpToken>, LexError> {
    lex_with_options(file, source, LexerOptions::default())
}

pub fn lex_with_options(
    file: FileId,
    source: &str,
    options: LexerOptions,
) -> Result<Vec<PpToken>, LexError> {
    let lexed = lex_file(file, source, options)?;
    if let Some(error) = lexed.line_errors.into_iter().flatten().next() {
        return Err(error);
    }
    Ok(lexed.lines.into_iter().flatten().collect())
}

pub(crate) fn lex_file(
    file: FileId,
    source: &str,
    options: LexerOptions,
) -> Result<LexedFile, LexError> {
    let normalized = normalize(
        file,
        source,
        NormalizeOptions {
            trigraphs: options.trigraphs,
            warn_trigraphs: options.warn_trigraphs,
        },
    );
    let mut lexer = Lexer::new(file, &normalized);
    lexer.run()?;
    Ok(LexedFile {
        lines: std::mem::take(&mut lexer.lines),
        line_errors: std::mem::take(&mut lexer.line_errors),
        diagnostics: normalized.diagnostics,
    })
}

struct Lexer<'a> {
    file: FileId,
    source: &'a NormalizedSource,
    index: usize,
    line: usize,
    at_start_of_line: bool,
    pending_space: bool,
    lines: Vec<Vec<PpToken>>,
    line_errors: Vec<Vec<LexError>>,
}

impl<'a> Lexer<'a> {
    fn new(file: FileId, source: &'a NormalizedSource) -> Self {
        Self {
            file,
            source,
            index: 0,
            line: 1,
            at_start_of_line: true,
            pending_space: false,
            lines: vec![Vec::new()],
            line_errors: vec![Vec::new()],
        }
    }

    fn run(&mut self) -> Result<(), LexError> {
        while self.index < self.source.text.len() {
            if self.consume_whitespace_or_comment()? {
                continue;
            }
            let start = self.index;
            match self.next_token() {
                Ok(token) => {
                    self.lines
                        .last_mut()
                        .expect("a line always exists")
                        .push(token);
                    self.at_start_of_line = false;
                    self.pending_space = false;
                }
                Err(error) => {
                    let skip_remainder = error.code == UNTERMINATED_LITERAL.as_str();
                    self.line_errors
                        .last_mut()
                        .expect("an error line always exists")
                        .push(error);
                    if skip_remainder {
                        while self.index < self.source.text.len()
                            && self.source.text.as_bytes()[self.index] != b'\n'
                        {
                            self.index += self.source.text[self.index..]
                                .chars()
                                .next()
                                .expect("index is valid")
                                .len_utf8();
                        }
                    } else if self.index == start {
                        self.index += self.source.text[self.index..]
                            .chars()
                            .next()
                            .expect("index is valid")
                            .len_utf8();
                    }
                }
            }
        }
        Ok(())
    }

    fn consume_whitespace_or_comment(&mut self) -> Result<bool, LexError> {
        let rest = &self.source.text[self.index..];
        let byte = rest.as_bytes()[0];
        if byte == b'\n' {
            self.index += 1;
            self.line += 1;
            self.at_start_of_line = true;
            self.pending_space = false;
            self.lines.push(Vec::new());
            self.line_errors.push(Vec::new());
            return Ok(true);
        }
        if byte.is_ascii_whitespace() || byte == b'\x0b' {
            self.pending_space = true;
            self.index += 1;
            return Ok(true);
        }
        if rest.starts_with("//") {
            self.pending_space = true;
            self.index += 2;
            while self.index < self.source.text.len()
                && self.source.text.as_bytes()[self.index] != b'\n'
            {
                self.index += self.source.text[self.index..]
                    .chars()
                    .next()
                    .expect("index is valid")
                    .len_utf8();
            }
            return Ok(true);
        }
        if rest.starts_with("/*") {
            let start = self.index;
            self.pending_space = true;
            self.index += 2;
            while self.index < self.source.text.len() {
                if self.source.text[self.index..].starts_with("*/") {
                    self.index += 2;
                    return Ok(true);
                }
                if self.source.text.as_bytes()[self.index] == b'\n' {
                    self.index += 1;
                    self.line += 1;
                } else {
                    self.index += self.source.text[self.index..]
                        .chars()
                        .next()
                        .expect("index is valid")
                        .len_utf8();
                }
            }
            return Err(self.error(
                "CCC0001",
                start,
                self.source.text.len(),
                "unterminated block comment",
            ));
        }
        Ok(false)
    }

    fn next_token(&mut self) -> Result<PpToken, LexError> {
        let start = self.index;
        let rest = &self.source.text[start..];
        let kind = if let Some(literal) = scan_prefixed_literal(rest).map_err(|()| {
            self.error(
                UNTERMINATED_LITERAL.as_str(),
                start,
                self.source
                    .text
                    .len()
                    .min(start + rest.find('\n').unwrap_or(rest.len())),
                "unterminated literal",
            )
        })? {
            self.index += literal.length;
            if literal.invalid_utf8_character {
                return Err(self.error(
                    "CCC0005",
                    start,
                    self.index,
                    "the u8 prefix is not valid on a character constant",
                ));
            }
            literal.kind
        } else if let Some(end) = scan_identifier(rest)? {
            self.index += end;
            let spelling = &rest[..end];
            if canonicalize_identifier(spelling).is_none() {
                return Err(self.error(
                    "CCC0004",
                    start,
                    self.index,
                    "invalid universal character name in identifier",
                ));
            }
            PpTokenKind::Identifier
        } else if rest.as_bytes()[0].is_ascii_digit()
            || (rest.starts_with('.') && rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
        {
            self.index += scan_pp_number(rest);
            PpTokenKind::PpNumber
        } else if matches!(rest.as_bytes()[0], b'\'' | b'"') {
            self.index += scan_quoted(rest, rest.as_bytes()[0]).map_err(|()| {
                self.error(
                    UNTERMINATED_LITERAL.as_str(),
                    start,
                    self.source
                        .text
                        .len()
                        .min(start + rest.find('\n').unwrap_or(rest.len())),
                    "unterminated literal",
                )
            })?;
            if rest.as_bytes()[0] == b'\'' {
                PpTokenKind::CharacterConstant
            } else {
                PpTokenKind::StringLiteral
            }
        } else if let Some(length) = punctuator_length(rest) {
            self.index += length;
            PpTokenKind::Punctuator
        } else {
            let character_length = rest.chars().next().expect("index is valid").len_utf8();
            self.index += character_length;
            PpTokenKind::Punctuator
        };

        if kind == PpTokenKind::CharacterConstant
            && let Err(error) =
                validate_character_constant_ucns(&self.source.text[start..self.index])
        {
            return Err(self.error("CCC0006", start, self.index, &error.message));
        }

        Ok(PpToken::direct(
            kind,
            self.source.original_span(self.file, start, self.index),
            self.source.text[start..self.index].to_owned(),
            self.pending_space,
            self.at_start_of_line,
            self.source.physical_line(start),
        ))
    }

    fn error(&self, code: &'static str, start: usize, end: usize, message: &str) -> LexError {
        LexError {
            code,
            span: self.source.original_span(self.file, start, end),
            message: message.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrefixedLiteral {
    length: usize,
    kind: PpTokenKind,
    invalid_utf8_character: bool,
}

fn scan_prefixed_literal(rest: &str) -> Result<Option<PrefixedLiteral>, ()> {
    for prefix in ["u8", "u", "U", "L"] {
        if let Some(tail) = rest.strip_prefix(prefix)
            && let Some(quote) = tail
                .as_bytes()
                .first()
                .copied()
                .filter(|b| matches!(b, b'\'' | b'"'))
        {
            return scan_quoted(tail, quote).map(|length| {
                Some(PrefixedLiteral {
                    length: prefix.len() + length,
                    kind: if quote == b'\'' {
                        PpTokenKind::CharacterConstant
                    } else {
                        PpTokenKind::StringLiteral
                    },
                    invalid_utf8_character: prefix == "u8" && quote == b'\'',
                })
            });
        }
    }
    Ok(None)
}

fn scan_quoted(rest: &str, quote: u8) -> Result<usize, ()> {
    let bytes = rest.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => {
                index += 1;
                index += rest[index..]
                    .chars()
                    .next()
                    .expect("the escaped character exists")
                    .len_utf8();
            }
            byte if byte == quote => return Ok(index + 1),
            b'\n' => break,
            _ => {
                index += rest[index..]
                    .chars()
                    .next()
                    .expect("index is valid")
                    .len_utf8();
            }
        }
    }
    Err(())
}

fn scan_identifier(rest: &str) -> Result<Option<usize>, LexError> {
    let Some((length, first)) = scan_identifier_character(rest) else {
        return Ok(None);
    };
    if !is_identifier_start(first) {
        return Ok(None);
    }
    let mut index = length;
    while index < rest.len() {
        let Some((length, character)) = scan_identifier_character(&rest[index..]) else {
            break;
        };
        if !is_identifier_continue(character) {
            break;
        }
        index += length;
    }
    Ok(Some(index))
}

fn scan_identifier_character(rest: &str) -> Option<(usize, char)> {
    if rest.starts_with("\\u") || rest.starts_with("\\U") {
        let digit_count = if rest.starts_with("\\u") { 4 } else { 8 };
        let digits = rest.as_bytes().get(2..2 + digit_count)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let digits = std::str::from_utf8(digits).ok()?;
        let value = u32::from_str_radix(digits, 16).ok()?;
        return char::from_u32(value).map(|character| (2 + digit_count, character));
    }
    let character = rest.chars().next()?;
    Some((character.len_utf8(), character))
}

fn is_identifier_start(character: char) -> bool {
    matches!(character, '_' | '$') || character.is_alphabetic() || (character as u32) >= 0x80
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit() || character.is_numeric()
}

fn scan_pp_number(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphanumeric()
            || matches!(bytes[index], b'.' | b'_')
            || (matches!(bytes[index], b'+' | b'-')
                && index > 0
                && matches!(bytes[index - 1], b'e' | b'E' | b'p' | b'P'))
        {
            index += 1;
        } else if rest[index..].starts_with("\\u") || rest[index..].starts_with("\\U") {
            let length = if rest[index..].starts_with("\\u") {
                6
            } else {
                10
            };
            if rest
                .as_bytes()
                .get(index + 2..index + length)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            {
                index += length;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    index
}

const MULTI_CHARACTER_PUNCTUATORS: &[&str] = &[
    "%:%:", "<<=", ">>=", "...", "->", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||",
    "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "##", "<:", ":>", "<%", "%>", "%:",
];
const SINGLE_CHARACTER_PUNCTUATORS: &str = "[](){}.,;:?~!+-*/%&|^<>=#";

pub(crate) fn punctuator_length(rest: &str) -> Option<usize> {
    MULTI_CHARACTER_PUNCTUATORS
        .iter()
        .find(|punctuator| rest.starts_with(**punctuator))
        .map(|punctuator| punctuator.len())
        .or_else(|| {
            rest.chars()
                .next()
                .filter(|character| SINGLE_CHARACTER_PUNCTUATORS.contains(*character))
                .map(char::len_utf8)
        })
}

/// Whether rendering two preprocessing tokens without whitespace could change
/// their tokenization. This mirrors the lexer boundary rules without building
/// and normalizing a temporary source file for every adjacent pair.
pub(crate) fn tokens_require_separator(left: &PpToken, right: &PpToken) -> bool {
    if left.spelling == "/" && matches!(right.spelling.as_bytes().first(), Some(b'/' | b'*')) {
        return true;
    }
    // Keep rendered output from creating a trigraph when a third token follows.
    if left.spelling == "?" && right.spelling == "?" {
        return true;
    }
    if left.spelling == "\\" && starts_universal_character_name_tail(&right.spelling) {
        return true;
    }

    match left.kind {
        PpTokenKind::Identifier => {
            scan_identifier_character(&right.spelling)
                .is_some_and(|(_, character)| is_identifier_continue(character))
                || (matches!(left.spelling.as_str(), "L" | "u" | "U" | "u8")
                    && matches!(right.spelling.as_bytes().first(), Some(b'\'' | b'"')))
        }
        PpTokenKind::PpNumber => {
            right
                .spelling
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
                || right.spelling.starts_with("\\u")
                || right.spelling.starts_with("\\U")
                || (matches!(right.spelling.as_bytes().first(), Some(b'+' | b'-'))
                    && matches!(
                        left.spelling.as_bytes().last(),
                        Some(b'e' | b'E' | b'p' | b'P')
                    ))
        }
        PpTokenKind::Punctuator => {
            if left.spelling == "."
                && right.kind == PpTokenKind::PpNumber
                && right
                    .spelling
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_digit)
            {
                return true;
            }
            MULTI_CHARACTER_PUNCTUATORS.iter().any(|punctuator| {
                punctuator.len() > left.spelling.len()
                    && punctuator.starts_with(&left.spelling)
                    && right
                        .spelling
                        .starts_with(&punctuator[left.spelling.len()..])
            })
        }
        PpTokenKind::StringLiteral | PpTokenKind::CharacterConstant => false,
    }
}

fn starts_universal_character_name_tail(spelling: &str) -> bool {
    let (digits, count) = if let Some(digits) = spelling.strip_prefix('u') {
        (digits, 4)
    } else if let Some(digits) = spelling.strip_prefix('U') {
        (digits, 8)
    } else {
        return false;
    };
    digits
        .as_bytes()
        .get(..count)
        .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
}

/// Relexes a pasted spelling and accepts it only when it forms exactly one token.
pub(crate) fn relex_one(reference: &PpToken, spelling: &str) -> Option<PpToken> {
    let mut sources = ccc_session::SourceMap::new();
    let file = sources.add_file("<paste>", spelling);
    let mut tokens = lex(file, spelling).ok()?;
    if tokens.len() != 1 {
        return None;
    }
    let mut token = tokens.remove(0);
    token.span = reference.span;
    token.logical_line = reference.logical_line;
    token.is_system_header = reference.is_system_header;
    Some(token)
}

pub(crate) fn lex_fragment(reference: Span, spelling: &str) -> Result<Vec<PpToken>, LexError> {
    let mut sources = ccc_session::SourceMap::new();
    let file = sources.add_file("<preprocessor-fragment>", spelling);
    let mut tokens = lex(file, spelling)?;
    for token in &mut tokens {
        token.span = reference;
        token.logical_line = 1;
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

    use super::*;

    #[test]
    fn forms_tokens_and_tracks_spacing() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int /* ignored */ main(void) {\n return 42; }");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();
        assert_eq!(
            spellings,
            [
                "int", "main", "(", "void", ")", "{", "return", "42", ";", "}"
            ]
        );
        assert!(tokens[1].leading_space);
        assert!(tokens[6].at_start_of_line);
    }

    #[test]
    fn accepts_utf8_and_ucn_identifiers() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "café caf\\u00e9");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        assert_eq!(tokens[0].identifier_key(), "café");
        assert_eq!(tokens[1].identifier_key(), "café");
        assert_eq!(tokens[0].span, Span::new(file, 0, "café".len()));
    }

    #[test]
    fn preserves_physical_line_across_splicing() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "\\\n__LINE__ __LI\\\nNE__");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        assert_eq!(tokens[0].logical_line, 2);
        assert_eq!(tokens[1].logical_line, 2);
    }

    #[test]
    fn rejects_an_unterminated_block_comment() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "/*");
        let error = lex(file, sources.source(file).unwrap()).unwrap_err();
        assert_eq!(error.code, "CCC0001");
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
    fn scans_escaped_multibyte_characters_at_utf8_boundaries() {
        let mut sources = SourceMap::new();
        for spelling in [
            r#""\é""#,
            r#"'\ä'"#,
            r#"u8"\λ""#,
            r#"L"\λ""#,
            r#"L'\λ'"#,
            r#"u"\λ""#,
            r#"u'\λ'"#,
            r#"U"\λ""#,
            r#"U'\λ'"#,
        ] {
            let file = sources.add_file("literal.c", spelling);
            let tokens = lex(file, spelling).unwrap();
            assert_eq!(tokens.len(), 1, "{spelling}");
            assert_eq!(tokens[0].spelling, spelling);
        }
    }

    #[test]
    fn rejects_utf8_prefixed_character_constants() {
        let mut sources = SourceMap::new();
        let source = "u8'a'";
        let file = sources.add_file("test.c", source);
        let error = lex(file, source).unwrap_err();
        assert_eq!(error.code, "CCC0005");
    }

    #[test]
    fn rejects_disallowed_ucns_in_character_constants() {
        let mut sources = SourceMap::new();
        for spelling in [r"'\u0041'", r"L'\u0041'", r"u'\u0041'", r"U'\u0041'"] {
            let file = sources.add_file("test.c", spelling);
            let error = lex(file, spelling).unwrap_err();
            assert_eq!(error.code, "CCC0006", "{spelling}");
        }

        let spelling = r"'\\u0041'";
        let file = sources.add_file("test.c", spelling);
        assert!(lex(file, spelling).is_ok());
    }

    #[test]
    fn forms_catch_all_tokens_and_accepts_dollar_in_identifiers() {
        let mut sources = SourceMap::new();
        let source = r"@ \ ` $ $name name$tail";
        let file = sources.add_file("test.c", source);
        let tokens = lex(file, source).unwrap();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();
        assert_eq!(spellings, ["@", "\\", "`", "$", "$name", "name$tail"]);
        assert_eq!(tokens[0].kind, PpTokenKind::Punctuator);
        assert_eq!(tokens[1].kind, PpTokenKind::Punctuator);
        assert_eq!(tokens[2].kind, PpTokenKind::Punctuator);
        assert!(
            tokens[3..]
                .iter()
                .all(|token| token.kind == PpTokenKind::Identifier)
        );
    }

    #[test]
    fn block_comment_newlines_do_not_end_a_directive_line() {
        let mut sources = SourceMap::new();
        let source = "#define X /* first\nsecond */ 1\nX\n";
        let file = sources.add_file("test.c", source);
        let lexed = lex_file(file, source, LexerOptions::default()).unwrap();
        assert_eq!(lexed.lines.len(), 3);
        let first_line: Vec<_> = lexed.lines[0]
            .iter()
            .map(|token| token.spelling.as_str())
            .collect();
        assert_eq!(first_line, ["#", "define", "X", "1"]);
        assert!(lexed.lines[0][3].leading_space);
        assert!(!lexed.lines[0][3].at_start_of_line);
        assert_eq!(lexed.lines[0][3].logical_line, 2);
        assert_eq!(lexed.lines[1][0].spelling, "X");
        assert!(lexed.lines[1][0].at_start_of_line);
        assert_eq!(lexed.lines[1][0].logical_line, 3);
    }

    #[test]
    fn treats_vertical_tab_as_whitespace() {
        let mut sources = SourceMap::new();
        let source = "#define\u{000b}X\u{000b}1\nX";
        let file = sources.add_file("test.c", source);
        let tokens = lex(file, source).unwrap();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();
        assert_eq!(spellings, ["#", "define", "X", "1", "X"]);
        assert!(tokens[2].leading_space);
        assert!(tokens[3].leading_space);
    }

    #[test]
    fn classifies_rendering_boundaries_without_relexing() {
        let merging = [
            ("name", "tail"),
            ("name", "42"),
            ("L", "\"text\""),
            ("name", "L\"text\""),
            ("1", "value"),
            ("1", ".5"),
            ("1e", "+"),
            ("1", "L\"text\""),
            (".", "5"),
            ("/", "*"),
            ("/", "/"),
            ("<", "<"),
            ("<<", "="),
            ("%:", "%:"),
            ("\\", "u00e9"),
            ("$", "name"),
        ];
        let separate = [
            ("name", ".5"),
            ("1", "$name"),
            ("\"text\"", "name"),
            ("+", "name"),
            ("@", "name"),
            (".", ".5"),
            ("\\", "plain"),
        ];
        let mut sources = SourceMap::new();

        for (left, right, expected) in merging
            .into_iter()
            .map(|(left, right)| (left, right, true))
            .chain(
                separate
                    .into_iter()
                    .map(|(left, right)| (left, right, false)),
            )
        {
            let left_token = one_token(&mut sources, left);
            let right_token = one_token(&mut sources, right);
            assert_eq!(
                tokens_require_separator(&left_token, &right_token),
                expected,
                "boundary {left:?} {right:?}"
            );

            let joined = format!("{left}{right}");
            let file = sources.add_file("joined.c", &joined);
            let changed = match lex(file, &joined) {
                Ok(tokens) => {
                    tokens.len() != 2 || tokens[0].spelling != left || tokens[1].spelling != right
                }
                Err(_) => true,
            };
            assert_eq!(changed, expected, "relexed boundary {left:?} {right:?}");
        }

        let question = one_token(&mut sources, "?");
        assert!(tokens_require_separator(&question, &question));
    }

    #[test]
    fn rendering_boundary_classifier_has_no_false_negatives() {
        let mut spellings = vec![
            "name".to_owned(),
            "L".to_owned(),
            "u".to_owned(),
            "U".to_owned(),
            "u8".to_owned(),
            "$name".to_owned(),
            "café".to_owned(),
            "\\u00e9".to_owned(),
            "0".to_owned(),
            "1e".to_owned(),
            ".5".to_owned(),
            "1e+2".to_owned(),
            "0x1p-2".to_owned(),
            "\"text\"".to_owned(),
            "L\"text\"".to_owned(),
            "u\"text\"".to_owned(),
            "U\"text\"".to_owned(),
            "u8\"text\"".to_owned(),
            "'x'".to_owned(),
            "L'x'".to_owned(),
            "u'x'".to_owned(),
            "U'x'".to_owned(),
            "@".to_owned(),
            "`".to_owned(),
            "\\".to_owned(),
        ];
        spellings.extend(
            MULTI_CHARACTER_PUNCTUATORS
                .iter()
                .map(|spelling| (*spelling).to_owned()),
        );
        spellings.extend(
            SINGLE_CHARACTER_PUNCTUATORS
                .chars()
                .map(|character| character.to_string()),
        );

        let mut sources = SourceMap::new();
        let tokens = spellings
            .iter()
            .map(|spelling| one_token(&mut sources, spelling))
            .collect::<Vec<_>>();
        for (left_spelling, left) in spellings.iter().zip(&tokens) {
            for (right_spelling, right) in spellings.iter().zip(&tokens) {
                let joined = format!("{left_spelling}{right_spelling}");
                let file = sources.add_file("joined.c", &joined);
                let changed = match lex(file, &joined) {
                    Ok(joined_tokens) => {
                        joined_tokens.len() != 2
                            || joined_tokens[0].spelling != *left_spelling
                            || joined_tokens[1].spelling != *right_spelling
                    }
                    Err(_) => true,
                };
                assert!(
                    !changed || tokens_require_separator(left, right),
                    "missed boundary {left_spelling:?} {right_spelling:?}"
                );
            }
        }
    }

    fn one_token(sources: &mut SourceMap, spelling: &str) -> PpToken {
        let file = sources.add_file("boundary.c", spelling);
        let mut tokens = lex(file, spelling).unwrap();
        assert_eq!(tokens.len(), 1, "{spelling:?} did not form one token");
        tokens.remove(0)
    }
}
