use crate::diagnostic::PpDiagnostic;
use crate::literal::{CharacterConstantPrefix, decode_character_constant, decode_integer_constant};
use crate::macros::{ExpansionLocation, MacroTable, expand_condition};
use crate::options::PreprocessOptions;
use crate::token::{PpToken, PpTokenKind};
use ccc_session::SourceMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct ConditionResult {
    pub value: bool,
    pub diagnostics: Vec<PpDiagnostic>,
}

pub(crate) fn evaluate<F>(
    sources: &mut SourceMap,
    table: &mut MacroTable,
    tokens: &[PpToken],
    options: &PreprocessOptions,
    wchar_is_signed: bool,
    location: ExpansionLocation<'_>,
    mut has_include: F,
) -> ConditionResult
where
    F: FnMut(&str, bool, bool) -> bool,
{
    let direct_tokens = replace_direct_header_predicates(tokens, table, &mut has_include);
    let expanded = expand_condition(sources, table, &direct_tokens, options, location);
    let (defined_tokens, mut diagnostics) = replace_defined(table, &expanded.tokens);
    diagnostics.extend(expanded.diagnostics);
    let (predicate_tokens, predicate_diagnostics) =
        replace_predicates(defined_tokens, table, options, &mut has_include);
    diagnostics.extend(predicate_diagnostics);
    let normalized = predicate_tokens
        .into_iter()
        .map(|mut token| {
            if token.kind == PpTokenKind::Identifier {
                token.kind = PpTokenKind::PpNumber;
                token.spelling = "0".to_owned();
            }
            token
        })
        .collect::<Vec<_>>();
    let mut parser = ExpressionParser {
        tokens: &normalized,
        index: 0,
        diagnostics,
        wchar_is_signed,
    };
    let value = parser.parse_conditional(true);
    if parser.index < parser.tokens.len() {
        parser.error(
            parser.index,
            "CCC1201",
            "unexpected token in conditional expression",
        );
    }
    ConditionResult {
        value: value.truthy(),
        diagnostics: parser.diagnostics,
    }
}

fn replace_direct_header_predicates<F>(
    tokens: &[PpToken],
    table: &MacroTable,
    has_include: &mut F,
) -> Vec<PpToken>
where
    F: FnMut(&str, bool, bool) -> bool,
{
    let mut output = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let predicate = (token.kind == PpTokenKind::Identifier)
            .then(|| token.identifier_key())
            .and_then(|name| header_predicate(&name).map(|include_next| (name, include_next)));
        let Some((predicate, include_next)) = predicate else {
            output.push(token.clone());
            index += 1;
            continue;
        };
        if !table.contains(&predicate)
            || table.get(&predicate).is_some()
            || tokens
                .get(index + 1)
                .is_none_or(|next| next.spelling != "(")
        {
            output.push(token.clone());
            index += 1;
            continue;
        }
        let Some(close) = matching_close(tokens, index + 1) else {
            output.push(token.clone());
            index += 1;
            continue;
        };
        let Some((header, angled)) = parse_header_operand(&tokens[index + 2..close]) else {
            output.push(token.clone());
            index += 1;
            continue;
        };
        output.push(predicate_result(
            token,
            has_include(&header, angled, include_next),
        ));
        index = close + 1;
    }
    output
}

fn replace_defined(table: &MacroTable, tokens: &[PpToken]) -> (Vec<PpToken>, Vec<PpDiagnostic>) {
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].kind == PpTokenKind::Identifier
            && tokens[index].identifier_key() == "defined"
        {
            let reference = &tokens[index];
            let parenthesized = tokens
                .get(index + 1)
                .is_some_and(|token| token.spelling == "(");
            let name_index = index + if parenthesized { 2 } else { 1 };
            let Some(name) = tokens
                .get(name_index)
                .filter(|token| token.kind == PpTokenKind::Identifier)
            else {
                diagnostics.push(
                    PpDiagnostic::error("CCC1202", "defined requires an identifier operand")
                        .with_span(reference.span),
                );
                index += 1;
                continue;
            };
            let end = if parenthesized {
                if tokens
                    .get(name_index + 1)
                    .is_none_or(|token| token.spelling != ")")
                {
                    diagnostics.push(
                        PpDiagnostic::error("CCC1203", "missing ')' after defined operand")
                            .with_span(reference.span),
                    );
                    index += 1;
                    continue;
                }
                name_index + 2
            } else {
                name_index + 1
            };
            let mut result = PpToken::synthetic(
                PpTokenKind::PpNumber,
                reference.span,
                if table.contains(&name.identifier_key()) {
                    "1"
                } else {
                    "0"
                },
            );
            result.leading_space = reference.leading_space;
            result.at_start_of_line = reference.at_start_of_line;
            result.logical_line = reference.logical_line;
            output.push(result);
            index = end;
            continue;
        }
        output.push(tokens[index].clone());
        index += 1;
    }
    (output, diagnostics)
}

fn replace_predicates<F>(
    tokens: Vec<PpToken>,
    table: &MacroTable,
    options: &PreprocessOptions,
    has_include: &mut F,
) -> (Vec<PpToken>, Vec<PpDiagnostic>)
where
    F: FnMut(&str, bool, bool) -> bool,
{
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let predicate = (token.kind == PpTokenKind::Identifier).then(|| token.identifier_key());
        let Some(predicate) = predicate.filter(|name| {
            matches!(
                name.as_str(),
                "__has_include"
                    | "__has_include_next"
                    | "__has_attribute"
                    | "__has_builtin"
                    | "__has_feature"
                    | "__has_extension"
            )
        }) else {
            output.push(token.clone());
            index += 1;
            continue;
        };
        if !table.contains(&predicate) {
            output.push(token.clone());
            index += 1;
            continue;
        }
        if tokens
            .get(index + 1)
            .is_none_or(|next| next.spelling != "(")
        {
            diagnostics.push(
                PpDiagnostic::error("CCC1204", format!("{predicate} requires parentheses"))
                    .with_span(token.span),
            );
            output.push(token.clone());
            index += 1;
            continue;
        }
        let Some(close) = matching_close(&tokens, index + 1) else {
            diagnostics.push(
                PpDiagnostic::error("CCC1205", format!("unterminated {predicate} invocation"))
                    .with_span(token.span),
            );
            output.push(token.clone());
            index += 1;
            continue;
        };
        let operand = &tokens[index + 2..close];
        let value = if let Some(include_next) = header_predicate(&predicate) {
            parse_header_operand(operand)
                .is_some_and(|(name, angled)| has_include(&name, angled, include_next))
        } else {
            let feature_name = operand
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<String>();
            let family = match predicate.as_str() {
                "__has_attribute" => "attribute",
                "__has_builtin" => "builtin",
                "__has_feature" => "feature",
                "__has_extension" => "extension",
                _ => unreachable!(),
            };
            options
                .features
                .get(&feature_name)
                .or_else(|| options.features.get(&format!("{family}:{feature_name}")))
                .copied()
                .unwrap_or(false)
        };
        output.push(predicate_result(token, value));
        index = close + 1;
    }
    (output, diagnostics)
}

fn header_predicate(name: &str) -> Option<bool> {
    match name {
        "__has_include" => Some(false),
        "__has_include_next" => Some(true),
        _ => None,
    }
}

fn predicate_result(reference: &PpToken, value: bool) -> PpToken {
    let mut result = PpToken::synthetic(
        PpTokenKind::PpNumber,
        reference.span,
        if value { "1" } else { "0" },
    );
    result.leading_space = reference.leading_space;
    result.at_start_of_line = reference.at_start_of_line;
    result.logical_line = reference.logical_line;
    result
}

fn matching_close(tokens: &[PpToken], open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.spelling.as_str() {
            "(" => depth += 1,
            ")" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn parse_header_operand(tokens: &[PpToken]) -> Option<(String, bool)> {
    if tokens.len() == 1 && tokens[0].kind == PpTokenKind::StringLiteral {
        let spelling = &tokens[0].spelling;
        let body = spelling.strip_prefix('"')?.strip_suffix('"')?;
        return Some((body.to_owned(), false));
    }
    if tokens.first()?.spelling == "<" && tokens.last()?.spelling == ">" {
        let mut header = String::new();
        for (index, token) in tokens[1..tokens.len() - 1].iter().enumerate() {
            if index > 0 && token.leading_space {
                header.push(' ');
            }
            header.push_str(&token.spelling);
        }
        return Some((header, true));
    }
    None
}

#[derive(Clone, Copy, Debug, Default)]
struct Value {
    bits: u64,
    unsigned: bool,
}

impl Value {
    const fn signed(bits: i64) -> Self {
        Self {
            bits: bits as u64,
            unsigned: false,
        }
    }

    const fn boolean(value: bool) -> Self {
        Self::signed(value as i64)
    }

    const fn truthy(self) -> bool {
        self.bits != 0
    }

    const fn as_signed(self) -> i64 {
        self.bits as i64
    }
}

struct ExpressionParser<'a> {
    tokens: &'a [PpToken],
    index: usize,
    diagnostics: Vec<PpDiagnostic>,
    wchar_is_signed: bool,
}

impl ExpressionParser<'_> {
    fn parse_conditional(&mut self, evaluate: bool) -> Value {
        let condition = self.parse_logical_or(evaluate);
        if !self.consume("?") {
            return condition;
        }
        let when_true = self.parse_conditional(evaluate && condition.truthy());
        if !self.consume(":") {
            self.error(
                self.index,
                "CCC1206",
                "expected ':' in conditional expression",
            );
            return Value::default();
        }
        let when_false = self.parse_conditional(evaluate && !condition.truthy());
        if condition.truthy() {
            when_true
        } else {
            when_false
        }
    }

    fn parse_logical_or(&mut self, evaluate: bool) -> Value {
        let mut left = self.parse_logical_and(evaluate);
        while self.consume("||") {
            let right = self.parse_logical_and(evaluate && !left.truthy());
            left = Value::boolean(left.truthy() || right.truthy());
        }
        left
    }

    fn parse_logical_and(&mut self, evaluate: bool) -> Value {
        let mut left = self.parse_bit_or(evaluate);
        while self.consume("&&") {
            let right = self.parse_bit_or(evaluate && left.truthy());
            left = Value::boolean(left.truthy() && right.truthy());
        }
        left
    }

    fn parse_bit_or(&mut self, evaluate: bool) -> Value {
        self.parse_binary(evaluate, Self::parse_bit_xor, &["|"], bitwise)
    }

    fn parse_bit_xor(&mut self, evaluate: bool) -> Value {
        self.parse_binary(evaluate, Self::parse_bit_and, &["^"], bitwise)
    }

    fn parse_bit_and(&mut self, evaluate: bool) -> Value {
        self.parse_binary(evaluate, Self::parse_equality, &["&"], bitwise)
    }

    fn parse_equality(&mut self, evaluate: bool) -> Value {
        self.parse_binary(evaluate, Self::parse_relational, &["==", "!="], compare)
    }

    fn parse_relational(&mut self, evaluate: bool) -> Value {
        self.parse_binary(
            evaluate,
            Self::parse_shift,
            &["<", ">", "<=", ">="],
            compare,
        )
    }

    fn parse_shift(&mut self, evaluate: bool) -> Value {
        let mut left = self.parse_additive(evaluate);
        while let Some(operator) = self.consume_any(&["<<", ">>"]) {
            let right = self.parse_additive(evaluate);
            if evaluate && right.bits >= 64 {
                self.error(
                    self.index.saturating_sub(1),
                    "CCC1207",
                    "shift count is too large",
                );
                left = Value::default();
            } else {
                left = match operator {
                    "<<" => Value {
                        bits: left.bits.wrapping_shl(right.bits as u32),
                        unsigned: left.unsigned,
                    },
                    ">>" if left.unsigned => Value {
                        bits: left.bits.wrapping_shr(right.bits as u32),
                        unsigned: true,
                    },
                    ">>" => Value::signed(left.as_signed().wrapping_shr(right.bits as u32)),
                    _ => unreachable!(),
                };
            }
        }
        left
    }

    fn parse_additive(&mut self, evaluate: bool) -> Value {
        self.parse_binary(
            evaluate,
            Self::parse_multiplicative,
            &["+", "-"],
            arithmetic,
        )
    }

    fn parse_multiplicative(&mut self, evaluate: bool) -> Value {
        let mut left = self.parse_unary(evaluate);
        while let Some(operator) = self.consume_any(&["*", "/", "%"]) {
            let right = self.parse_unary(evaluate);
            if evaluate && matches!(operator, "/" | "%") && right.bits == 0 {
                self.error(
                    self.index.saturating_sub(1),
                    "CCC1208",
                    "division by zero in conditional expression",
                );
                left = Value::default();
            } else if right.bits != 0 || operator == "*" {
                left = arithmetic(left, right, operator);
            }
        }
        left
    }

    fn parse_unary(&mut self, evaluate: bool) -> Value {
        if let Some(operator) = self.consume_any(&["+", "-", "!", "~"]) {
            let operand = self.parse_unary(evaluate);
            return match operator {
                "+" => operand,
                "-" => Value {
                    bits: 0_u64.wrapping_sub(operand.bits),
                    unsigned: operand.unsigned,
                },
                "!" => Value::boolean(!operand.truthy()),
                "~" => Value {
                    bits: !operand.bits,
                    unsigned: operand.unsigned,
                },
                _ => unreachable!(),
            };
        }
        self.parse_primary(evaluate)
    }

    fn parse_primary(&mut self, evaluate: bool) -> Value {
        if self.consume("(") {
            let value = self.parse_conditional(evaluate);
            if !self.consume(")") {
                self.error(
                    self.index,
                    "CCC1209",
                    "expected ')' in conditional expression",
                );
            }
            return value;
        }
        let Some(token) = self.tokens.get(self.index) else {
            self.error(self.index, "CCC1210", "expected expression");
            return Value::default();
        };
        self.index += 1;
        match token.kind {
            PpTokenKind::PpNumber => match decode_integer_constant(&token.spelling) {
                Ok(integer) if integer.value <= u64::MAX as u128 => Value {
                    bits: integer.value as u64,
                    unsigned: integer.suffix.unsigned || integer.value > i64::MAX as u128,
                },
                Ok(_) => {
                    self.diagnostics.push(
                        PpDiagnostic::error(
                            "CCC1211",
                            "integer constant is too large for a preprocessor expression",
                        )
                        .with_span(token.span),
                    );
                    Value::default()
                }
                Err(error) => {
                    self.diagnostics
                        .push(PpDiagnostic::error("CCC1211", error.message).with_span(token.span));
                    Value::default()
                }
            },
            PpTokenKind::CharacterConstant => match decode_character_constant(&token.spelling) {
                Ok(character) => Value {
                    bits: character.value,
                    unsigned: match character.prefix {
                        CharacterConstantPrefix::None => false,
                        CharacterConstantPrefix::Wide => !self.wchar_is_signed,
                        CharacterConstantPrefix::Utf16 | CharacterConstantPrefix::Utf32 => true,
                    },
                },
                Err(error) => {
                    self.diagnostics
                        .push(PpDiagnostic::error("CCC1212", error.message).with_span(token.span));
                    Value::default()
                }
            },
            _ => {
                self.error(
                    self.index - 1,
                    "CCC1213",
                    "invalid token in conditional expression",
                );
                Value::default()
            }
        }
    }

    fn parse_binary(
        &mut self,
        evaluate: bool,
        operand: fn(&mut Self, bool) -> Value,
        operators: &[&'static str],
        operation: fn(Value, Value, &str) -> Value,
    ) -> Value {
        let mut left = operand(self, evaluate);
        while let Some(operator) = self.consume_any(operators) {
            let right = operand(self, evaluate);
            left = operation(left, right, operator);
        }
        left
    }

    fn consume(&mut self, spelling: &str) -> bool {
        if self
            .tokens
            .get(self.index)
            .is_some_and(|token| token.spelling == spelling)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn consume_any(&mut self, spellings: &[&'static str]) -> Option<&'static str> {
        let spelling = self.tokens.get(self.index)?.spelling.as_str();
        let matched = spellings
            .iter()
            .copied()
            .find(|candidate| *candidate == spelling)?;
        self.index += 1;
        Some(matched)
    }

    fn error(&mut self, index: usize, code: &'static str, message: &str) {
        let mut diagnostic = PpDiagnostic::error(code, message);
        if let Some(token) = self.tokens.get(index).or_else(|| self.tokens.last()) {
            diagnostic = diagnostic.with_span(token.span);
        }
        self.diagnostics.push(diagnostic);
    }
}

fn usual(left: Value, right: Value) -> (Value, Value, bool) {
    let unsigned = left.unsigned || right.unsigned;
    (
        Value { unsigned, ..left },
        Value { unsigned, ..right },
        unsigned,
    )
}

fn arithmetic(left: Value, right: Value, operator: &str) -> Value {
    let (left, right, unsigned) = usual(left, right);
    let bits = match operator {
        "+" => left.bits.wrapping_add(right.bits),
        "-" => left.bits.wrapping_sub(right.bits),
        "*" => left.bits.wrapping_mul(right.bits),
        "/" if unsigned => left.bits / right.bits,
        "%" if unsigned => left.bits % right.bits,
        "/" => left.as_signed().wrapping_div(right.as_signed()) as u64,
        "%" => left.as_signed().wrapping_rem(right.as_signed()) as u64,
        _ => unreachable!(),
    };
    Value { bits, unsigned }
}

fn bitwise(left: Value, right: Value, operator: &str) -> Value {
    let (left, right, unsigned) = usual(left, right);
    let bits = match operator {
        "|" => left.bits | right.bits,
        "^" => left.bits ^ right.bits,
        "&" => left.bits & right.bits,
        _ => unreachable!(),
    };
    Value { bits, unsigned }
}

fn compare(left: Value, right: Value, operator: &str) -> Value {
    let (left, right, unsigned) = usual(left, right);
    let result = if unsigned {
        match operator {
            "==" => left.bits == right.bits,
            "!=" => left.bits != right.bits,
            "<" => left.bits < right.bits,
            ">" => left.bits > right.bits,
            "<=" => left.bits <= right.bits,
            ">=" => left.bits >= right.bits,
            _ => unreachable!(),
        }
    } else {
        match operator {
            "==" => left.as_signed() == right.as_signed(),
            "!=" => left.as_signed() != right.as_signed(),
            "<" => left.as_signed() < right.as_signed(),
            ">" => left.as_signed() > right.as_signed(),
            "<=" => left.as_signed() <= right.as_signed(),
            ">=" => left.as_signed() >= right.as_signed(),
            _ => unreachable!(),
        }
    };
    Value::boolean(result)
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

    use super::*;
    use crate::lexer::lex;

    #[test]
    fn evaluates_precedence_defined_and_lazy_operators() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "defined(X) && (2 + 3 * 4 == 14) || (0 && 1/0)");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let mut table = MacroTable::default();
        table.define(crate::macros::MacroDefinition {
            name: "X".to_owned(),
            form: crate::macros::MacroForm::Object,
            replacement: Vec::new(),
            definition_span: tokens[0].span,
            predefined: false,
        });
        let result = evaluate(
            &mut sources,
            &mut table,
            &tokens,
            &PreprocessOptions::default(),
            true,
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
            |_, _, _| false,
        );
        assert!(result.value);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn evaluates_defined_operators_produced_by_macro_expansion() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "CHECK_PRESENT && !CHECK_MISSING && CHECK_BARE");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let mut table = MacroTable::default();
        let span = tokens[0].span;
        for (name, replacement) in [
            ("PRESENT", "1"),
            ("CHECK_PRESENT", "defined(PRESENT)"),
            ("CHECK_MISSING", "defined(MISSING)"),
            ("CHECK_BARE", "defined PRESENT"),
        ] {
            let replacement_file = sources.add_file(format!("{name}.replacement"), replacement);
            table.define(crate::macros::MacroDefinition {
                name: name.to_owned(),
                form: crate::macros::MacroForm::Object,
                replacement: lex(replacement_file, sources.source(replacement_file).unwrap())
                    .unwrap(),
                definition_span: span,
                predefined: false,
            });
        }
        let result = evaluate(
            &mut sources,
            &mut table,
            &tokens,
            &PreprocessOptions::default(),
            true,
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
            |_, _, _| false,
        );
        assert!(result.value);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn generated_defined_operator_obeys_the_output_token_limit() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "CHECK");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let replacement_file = sources.add_file("replacement", "defined(PRESENT)");
        let mut table = MacroTable::default();
        table.define(crate::macros::MacroDefinition {
            name: "CHECK".to_owned(),
            form: crate::macros::MacroForm::Object,
            replacement: lex(replacement_file, sources.source(replacement_file).unwrap()).unwrap(),
            definition_span: tokens[0].span,
            predefined: false,
        });
        let mut options = PreprocessOptions::default();
        options.limits.output_tokens = 3;
        let result = evaluate(
            &mut sources,
            &mut table,
            &tokens,
            &options,
            true,
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
            |_, _, _| false,
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CCC1102")
        );
    }
}
