use std::fmt;

use crate::token::is_disallowed_ucn;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntegerSuffix {
    pub unsigned: bool,
    pub long_count: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegerConstant {
    pub value: u128,
    pub radix: u32,
    pub suffix: IntegerSuffix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterConstant {
    pub value: u64,
    pub character_count: usize,
    pub prefix: CharacterConstantPrefix,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CharacterConstantPrefix {
    #[default]
    None,
    Wide,
    Utf16,
    Utf32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralError {
    pub message: String,
}

impl fmt::Display for LiteralError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for LiteralError {}

pub fn decode_integer_constant(spelling: &str) -> Result<IntegerConstant, LiteralError> {
    let suffix_start = spelling
        .char_indices()
        .rev()
        .take_while(|(_, character)| matches!(character, 'u' | 'U' | 'l' | 'L'))
        .last()
        .map_or(spelling.len(), |(index, _)| index);
    let digits = &spelling[..suffix_start];
    let suffix_text = &spelling[suffix_start..];
    let mut suffix = IntegerSuffix::default();
    let mut chars = suffix_text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            'u' | 'U' if !suffix.unsigned => suffix.unsigned = true,
            'l' | 'L' if suffix.long_count == 0 => {
                suffix.long_count = 1;
                if chars.peek().is_some_and(|next| *next == character) {
                    chars.next();
                    suffix.long_count = 2;
                }
            }
            _ => return Err(literal_error("invalid integer suffix")),
        }
    }

    let (radix, digits) = if let Some(digits) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, digits)
    } else if let Some(digits) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, digits)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };
    if digits.is_empty() {
        return Err(literal_error("integer constant has no digits"));
    }
    let value = u128::from_str_radix(digits, radix)
        .map_err(|_| literal_error("integer constant is invalid or too large"))?;
    Ok(IntegerConstant {
        value,
        radix,
        suffix,
    })
}

pub fn decode_character_constant(spelling: &str) -> Result<CharacterConstant, LiteralError> {
    if spelling.starts_with("u8'") {
        return Err(literal_error(
            "the u8 prefix is not valid on a character constant",
        ));
    }
    let (prefix, rest) = if let Some(rest) = spelling.strip_prefix('L') {
        (CharacterConstantPrefix::Wide, rest)
    } else if let Some(rest) = spelling.strip_prefix('u') {
        (CharacterConstantPrefix::Utf16, rest)
    } else if let Some(rest) = spelling.strip_prefix('U') {
        (CharacterConstantPrefix::Utf32, rest)
    } else {
        (CharacterConstantPrefix::None, spelling)
    };
    let body = rest
        .strip_prefix('\'')
        .and_then(|body| body.strip_suffix('\''))
        .ok_or_else(|| literal_error("invalid character constant"))?;
    let mut values = Vec::new();
    let mut index = 0;
    while index < body.len() {
        let (value, length) = if body.as_bytes()[index] == b'\\' {
            decode_escape(&body[index..])?
        } else {
            let character = body[index..].chars().next().expect("index is valid");
            (character as u32, character.len_utf8())
        };
        values.push(value);
        index += length;
    }
    if values.is_empty() {
        return Err(literal_error("empty character constant"));
    }
    let value = if prefix == CharacterConstantPrefix::None {
        values.iter().fold(0_u64, |accumulator, value| {
            accumulator.wrapping_shl(8) | u64::from(*value & 0xff)
        })
    } else {
        if values.len() != 1 {
            return Err(literal_error(
                "a prefixed character constant must contain exactly one character",
            ));
        }
        u64::from(values[0])
    };
    if prefix == CharacterConstantPrefix::Utf16 && value > u64::from(u16::MAX) {
        return Err(literal_error(
            "character is not representable in a UTF-16 code unit",
        ));
    }
    Ok(CharacterConstant {
        value,
        character_count: values.len(),
        prefix,
    })
}

pub(crate) fn validate_character_constant_ucns(spelling: &str) -> Result<(), LiteralError> {
    let body = spelling
        .find('\'')
        .and_then(|start| spelling.get(start + 1..spelling.len().checked_sub(1)?))
        .ok_or_else(|| literal_error("invalid character constant"))?;
    let mut index = 0;
    while index < body.len() {
        let character = body[index..].chars().next().expect("index is valid");
        if character != '\\' {
            index += character.len_utf8();
            continue;
        }
        let Some(marker) = body[index + 1..].chars().next() else {
            break;
        };
        if matches!(marker, 'u' | 'U') {
            let (_, length) = decode_universal_character_name(&body[index..], marker)?;
            index += length;
        } else {
            index += 1 + marker.len_utf8();
        }
    }
    Ok(())
}

fn decode_escape(rest: &str) -> Result<(u32, usize), LiteralError> {
    let mut chars = rest[1..].char_indices();
    let (_, character) = chars
        .next()
        .ok_or_else(|| literal_error("incomplete escape sequence"))?;
    let simple = match character {
        '\'' => Some(b'\''),
        '"' => Some(b'"'),
        '?' => Some(b'?'),
        '\\' => Some(b'\\'),
        'a' => Some(7),
        'b' => Some(8),
        'f' => Some(12),
        'n' => Some(b'\n'),
        'r' => Some(b'\r'),
        't' => Some(b'\t'),
        'v' => Some(11),
        _ => None,
    };
    if let Some(value) = simple {
        return Ok((u32::from(value), 2));
    }
    if matches!(character, 'x') {
        let count = rest[2..].bytes().take_while(u8::is_ascii_hexdigit).count();
        if count == 0 {
            return Err(literal_error("hex escape has no digits"));
        }
        let value = u32::from_str_radix(&rest[2..2 + count], 16)
            .map_err(|_| literal_error("hex escape is too large"))?;
        return Ok((value, 2 + count));
    }
    if matches!(character, 'u' | 'U') {
        return decode_universal_character_name(rest, character);
    }
    if matches!(character, '0'..='7') {
        let count = rest[1..]
            .bytes()
            .take(3)
            .take_while(|byte| matches!(byte, b'0'..=b'7'))
            .count();
        let value = u32::from_str_radix(&rest[1..1 + count], 8)
            .map_err(|_| literal_error("invalid octal escape"))?;
        return Ok((value, 1 + count));
    }
    Err(literal_error("unknown escape sequence"))
}

fn decode_universal_character_name(rest: &str, marker: char) -> Result<(u32, usize), LiteralError> {
    let count = if marker == 'u' { 4 } else { 8 };
    let digits = rest
        .as_bytes()
        .get(2..2 + count)
        .ok_or_else(|| literal_error("incomplete universal character name"))?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return Err(literal_error("invalid universal character name"));
    }
    let digits = std::str::from_utf8(digits)
        .map_err(|_| literal_error("invalid universal character name"))?;
    let value = u32::from_str_radix(digits, 16)
        .map_err(|_| literal_error("invalid universal character name"))?;
    let character =
        char::from_u32(value).ok_or_else(|| literal_error("invalid Unicode scalar value"))?;
    if is_disallowed_ucn(character) {
        return Err(literal_error(
            "universal character name names a disallowed character",
        ));
    }
    Ok((value, 2 + count))
}

fn literal_error(message: &str) -> LiteralError {
    LiteralError {
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_integer_bases_and_suffixes() {
        assert_eq!(decode_integer_constant("0xffUL").unwrap().value, 255);
        assert_eq!(decode_integer_constant("077").unwrap().value, 63);
        assert_eq!(decode_integer_constant("0b101").unwrap().value, 5);
    }

    #[test]
    fn decodes_character_escapes() {
        assert_eq!(decode_character_constant("'\\n'").unwrap().value, 10);
        assert_eq!(decode_character_constant("'\\x41'").unwrap().value, 65);
    }

    #[test]
    fn preserves_full_prefixed_character_values_and_prefixes() {
        for (spelling, prefix) in [
            ("L'中'", CharacterConstantPrefix::Wide),
            ("u'中'", CharacterConstantPrefix::Utf16),
            ("U'中'", CharacterConstantPrefix::Utf32),
        ] {
            let character = decode_character_constant(spelling).unwrap();
            assert_eq!(character.value, 20_013, "{spelling}");
            assert_eq!(character.prefix, prefix, "{spelling}");
            assert_eq!(character.character_count, 1, "{spelling}");
        }
    }

    #[test]
    fn rejects_utf8_character_constants_and_unrepresentable_utf16_values() {
        assert!(decode_character_constant("u8'a'").is_err());
        assert!(decode_character_constant("u'😀'").is_err());
    }

    #[test]
    fn rejects_disallowed_universal_character_names_in_character_constants() {
        for spelling in [r"'\u0041'", r"L'\u0041'", r"u'\u0041'", r"U'\u0041'"] {
            let error = decode_character_constant(spelling).unwrap_err();
            assert!(error.message.contains("disallowed"), "{spelling}: {error}");
        }

        for spelling in [r"L'\u0024'", r"u'\u0040'", r"U'\u0060'"] {
            assert!(decode_character_constant(spelling).is_ok(), "{spelling}");
        }

        assert!(decode_character_constant(r"'\u中中'").is_err());
    }
}
