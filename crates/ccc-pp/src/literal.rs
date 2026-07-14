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
pub enum FloatingConstantSuffix {
    Float,
    Double,
    LongDouble,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatingConstant {
    pub value: f64,
    pub radix: u32,
    pub suffix: FloatingConstantSuffix,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StringLiteralPrefix {
    #[default]
    None,
    Utf8,
    Wide,
    Utf16,
    Utf32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringLiteral {
    pub prefix: StringLiteralPrefix,
    /// Decoded code units without the implicit trailing null code unit.
    pub code_units: Vec<u32>,
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

pub fn decode_floating_constant(spelling: &str) -> Result<FloatingConstant, LiteralError> {
    let (number, suffix) = match spelling.as_bytes().last().copied() {
        Some(b'f' | b'F') => (
            &spelling[..spelling.len() - 1],
            FloatingConstantSuffix::Float,
        ),
        Some(b'l' | b'L') => (
            &spelling[..spelling.len() - 1],
            FloatingConstantSuffix::LongDouble,
        ),
        _ => (spelling, FloatingConstantSuffix::Double),
    };
    if number.is_empty() {
        return Err(literal_error("floating constant has no digits"));
    }

    let (value, radix) = if let Some(hexadecimal) = number
        .strip_prefix("0x")
        .or_else(|| number.strip_prefix("0X"))
    {
        (decode_hexadecimal_float(hexadecimal)?, 16)
    } else {
        if !number.contains('.') && !number.contains('e') && !number.contains('E') {
            return Err(literal_error(
                "decimal floating constant requires a decimal point or exponent",
            ));
        }
        let value = number
            .parse::<f64>()
            .map_err(|_| literal_error("invalid decimal floating constant"))?;
        (value, 10)
    };
    if !value.is_finite() {
        return Err(literal_error(
            "floating constant is outside the supported range",
        ));
    }
    Ok(FloatingConstant {
        value,
        radix,
        suffix,
    })
}

fn decode_hexadecimal_float(spelling: &str) -> Result<f64, LiteralError> {
    let exponent_index = spelling
        .find(['p', 'P'])
        .ok_or_else(|| literal_error("hexadecimal floating constant requires a binary exponent"))?;
    let mantissa = &spelling[..exponent_index];
    let exponent = spelling[exponent_index + 1..]
        .parse::<i32>()
        .map_err(|_| literal_error("invalid hexadecimal floating exponent"))?;
    let (integer, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |parts| parts);
    if integer.is_empty() && fraction.is_empty() {
        return Err(literal_error("hexadecimal floating constant has no digits"));
    }
    if !integer
        .bytes()
        .chain(fraction.bytes())
        .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(literal_error("invalid hexadecimal floating mantissa"));
    }

    let mut value = 0.0_f64;
    for byte in integer.bytes() {
        value = value * 16.0 + f64::from(hexadecimal_digit(byte));
    }
    let mut place = 1.0_f64 / 16.0;
    for byte in fraction.bytes() {
        value += f64::from(hexadecimal_digit(byte)) * place;
        place /= 16.0;
    }
    Ok(value * 2.0_f64.powi(exponent))
}

fn hexadecimal_digit(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("caller validates hexadecimal digits"),
    }
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

pub fn decode_string_literal(spelling: &str) -> Result<StringLiteral, LiteralError> {
    let (prefix, rest) = if let Some(rest) = spelling.strip_prefix("u8") {
        (StringLiteralPrefix::Utf8, rest)
    } else if let Some(rest) = spelling.strip_prefix('L') {
        (StringLiteralPrefix::Wide, rest)
    } else if let Some(rest) = spelling.strip_prefix('u') {
        (StringLiteralPrefix::Utf16, rest)
    } else if let Some(rest) = spelling.strip_prefix('U') {
        (StringLiteralPrefix::Utf32, rest)
    } else {
        (StringLiteralPrefix::None, spelling)
    };
    let body = rest
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
        .ok_or_else(|| literal_error("invalid string literal"))?;

    let mut code_units = Vec::new();
    let mut index = 0;
    while index < body.len() {
        let (value, length, character) = if body.as_bytes()[index] == b'\\' {
            let marker = body[index + 1..]
                .chars()
                .next()
                .ok_or_else(|| literal_error("incomplete escape sequence"))?;
            let (value, length) = decode_escape(&body[index..])?;
            (
                value,
                length,
                matches!(marker, 'u' | 'U')
                    .then(|| char::from_u32(value))
                    .flatten(),
            )
        } else {
            let character = body[index..].chars().next().expect("index is valid");
            (character as u32, character.len_utf8(), Some(character))
        };
        append_string_code_units(&mut code_units, prefix, value, character)?;
        index += length;
    }

    Ok(StringLiteral { prefix, code_units })
}

pub fn concatenate_string_literals(
    literals: &[StringLiteral],
) -> Result<StringLiteral, LiteralError> {
    let prefix = literals
        .iter()
        .map(|literal| literal.prefix)
        .filter(|prefix| *prefix != StringLiteralPrefix::None)
        .try_fold(None, |selected, prefix| match selected {
            None => Ok(Some(prefix)),
            Some(selected) if selected == prefix => Ok(Some(selected)),
            Some(_) => Err(literal_error(
                "adjacent string literals have incompatible encoding prefixes",
            )),
        })?
        .unwrap_or(StringLiteralPrefix::None);

    let mut code_units = Vec::new();
    for literal in literals {
        if literal.prefix == StringLiteralPrefix::None && prefix != StringLiteralPrefix::None {
            let spelling = String::from_utf8(
                literal
                    .code_units
                    .iter()
                    .map(|unit| u8::try_from(*unit))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| literal_error("ordinary string code unit is out of range"))?,
            )
            .map_err(|_| literal_error("ordinary string is not valid UTF-8"))?;
            for character in spelling.chars() {
                append_string_code_units(
                    &mut code_units,
                    prefix,
                    character as u32,
                    Some(character),
                )?;
            }
        } else {
            code_units.extend_from_slice(&literal.code_units);
        }
    }
    Ok(StringLiteral { prefix, code_units })
}

fn append_string_code_units(
    output: &mut Vec<u32>,
    prefix: StringLiteralPrefix,
    value: u32,
    character: Option<char>,
) -> Result<(), LiteralError> {
    match prefix {
        StringLiteralPrefix::None | StringLiteralPrefix::Utf8 => {
            if let Some(character) = character {
                let mut encoded = [0_u8; 4];
                output.extend(
                    character
                        .encode_utf8(&mut encoded)
                        .as_bytes()
                        .iter()
                        .copied()
                        .map(u32::from),
                );
            } else if value <= u32::from(u8::MAX) {
                output.push(value);
            } else {
                return Err(literal_error(
                    "escape value is not representable in an 8-bit string code unit",
                ));
            }
        }
        StringLiteralPrefix::Utf16 => {
            if let Some(character) = character {
                let mut encoded = [0_u16; 2];
                output.extend(
                    character
                        .encode_utf16(&mut encoded)
                        .iter()
                        .copied()
                        .map(u32::from),
                );
            } else if value <= u32::from(u16::MAX) {
                output.push(value);
            } else {
                return Err(literal_error(
                    "escape value is not representable in a UTF-16 code unit",
                ));
            }
        }
        StringLiteralPrefix::Wide | StringLiteralPrefix::Utf32 => output.push(value),
    }
    Ok(())
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
    fn decodes_decimal_and_hexadecimal_floating_constants() {
        assert_eq!(
            decode_floating_constant("1.25f").unwrap(),
            FloatingConstant {
                value: 1.25,
                radix: 10,
                suffix: FloatingConstantSuffix::Float,
            }
        );
        assert_eq!(decode_floating_constant("1e2").unwrap().value, 100.0);
        assert_eq!(decode_floating_constant("0x1.8p+2").unwrap().value, 6.0);
        assert_eq!(
            decode_floating_constant("0x1p-2L").unwrap(),
            FloatingConstant {
                value: 0.25,
                radix: 16,
                suffix: FloatingConstantSuffix::LongDouble,
            }
        );
    }

    #[test]
    fn rejects_malformed_floating_constants() {
        for spelling in ["1f", "0x1.0", "0xp1", "0x1p", "1e9999"] {
            assert!(decode_floating_constant(spelling).is_err(), "{spelling}");
        }
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

    #[test]
    fn decodes_string_prefixes_escapes_and_unicode_code_units() {
        assert_eq!(
            decode_string_literal(r#""A\n中""#).unwrap(),
            StringLiteral {
                prefix: StringLiteralPrefix::None,
                code_units: vec![65, 10, 0xe4, 0xb8, 0xad],
            }
        );
        assert_eq!(
            decode_string_literal(r#"u"A😀""#).unwrap().code_units,
            vec![65, 0xd83d, 0xde00]
        );
        assert_eq!(
            decode_string_literal(r#"U"A😀""#).unwrap().code_units,
            vec![65, 0x1f600]
        );
        assert_eq!(
            decode_string_literal(r#"u8"A😀""#).unwrap().code_units,
            vec![65, 0xf0, 0x9f, 0x98, 0x80]
        );
    }

    #[test]
    fn concatenates_ordinary_and_compatible_prefixed_strings() {
        let literals = [
            decode_string_literal(r#"u"left ""#).unwrap(),
            decode_string_literal(r#""right""#).unwrap(),
        ];
        assert_eq!(
            concatenate_string_literals(&literals).unwrap(),
            StringLiteral {
                prefix: StringLiteralPrefix::Utf16,
                code_units: "left right".encode_utf16().map(u32::from).collect(),
            }
        );

        let incompatible = [
            decode_string_literal(r#"u"left""#).unwrap(),
            decode_string_literal(r#"U"right""#).unwrap(),
        ];
        assert!(concatenate_string_literals(&incompatible).is_err());
    }

    #[test]
    fn rejects_string_escape_values_that_do_not_fit_the_element_type() {
        assert!(decode_string_literal(r#""\x100""#).is_err());
        assert!(decode_string_literal(r#"u"\x10000""#).is_err());
    }
}
