use std::collections::{BTreeMap, BTreeSet};

use ccc_session::{OriginKind, SourceMap, Span};

use crate::diagnostic::{PpDiagnostic, PpDiagnosticCategory, PpSeverity};
use crate::lexer::relex_one;
use crate::options::PreprocessOptions;
use crate::token::{PpToken, PpTokenKind, canonicalize_identifier};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MacroForm {
    Object,
    Function {
        parameters: Vec<String>,
        variadic: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MacroDefinition {
    pub name: String,
    pub form: MacroForm,
    pub replacement: Vec<PpToken>,
    pub definition_span: Span,
    pub predefined: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MacroTable {
    definitions: BTreeMap<String, MacroDefinition>,
    disabled_dynamic: BTreeSet<String>,
    counter: u64,
}

impl MacroTable {
    pub fn get(&self, name: &str) -> Option<&MacroDefinition> {
        self.definitions.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.definitions.contains_key(name) || self.dynamic_enabled(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<MacroDefinition> {
        if is_dynamic_name(name) {
            self.disabled_dynamic.insert(name.to_owned());
        }
        self.definitions.remove(name)
    }

    pub fn definitions(&self) -> impl Iterator<Item = (&str, &MacroDefinition)> {
        self.definitions
            .iter()
            .map(|(name, definition)| (name.as_str(), definition))
    }

    pub fn define(&mut self, definition: MacroDefinition) -> DefineResult {
        if is_dynamic_name(&definition.name) {
            self.disabled_dynamic.insert(definition.name.clone());
        }
        match self.definitions.get(&definition.name) {
            None => {
                self.definitions.insert(definition.name.clone(), definition);
                DefineResult::Inserted
            }
            Some(previous) if equivalent(previous, &definition) => DefineResult::Equivalent,
            Some(previous) => {
                let previous = previous.clone();
                self.definitions.insert(definition.name.clone(), definition);
                DefineResult::Replaced(previous)
            }
        }
    }

    fn next_counter(&mut self) -> u64 {
        let value = self.counter;
        self.counter = self.counter.saturating_add(1);
        value
    }

    fn dynamic_enabled(&self, name: &str) -> bool {
        is_dynamic_name(name) && !self.disabled_dynamic.contains(name)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DefineResult {
    Inserted,
    Equivalent,
    Replaced(MacroDefinition),
}

fn equivalent(left: &MacroDefinition, right: &MacroDefinition) -> bool {
    if left.form != right.form || left.replacement.len() != right.replacement.len() {
        return false;
    }
    left.replacement
        .iter()
        .zip(&right.replacement)
        .enumerate()
        .all(|(index, (left, right))| {
            left.kind == right.kind
                && left.spelling == right.spelling
                && (index == 0 || left.leading_space == right.leading_space)
        })
}

#[derive(Clone, Debug)]
pub(crate) struct ExpansionLocation<'a> {
    pub logical_file: &'a str,
    pub is_system_header: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExpansionResult {
    pub tokens: Vec<PpToken>,
    pub diagnostics: Vec<PpDiagnostic>,
    pub trailing_function_macro_can_continue: bool,
}

pub(crate) fn expand(
    sources: &mut SourceMap,
    table: &mut MacroTable,
    tokens: &[PpToken],
    options: &PreprocessOptions,
    location: ExpansionLocation<'_>,
) -> ExpansionResult {
    let mut expander = Expander {
        sources,
        table,
        options,
        location,
        diagnostics: Vec::new(),
        emitted_tokens: 0,
    };
    let expanded = expander.expand_sequence(tokens);
    ExpansionResult {
        tokens: expanded.tokens,
        diagnostics: expander.diagnostics,
        trailing_function_macro_can_continue: expanded.trailing_function_macro_can_continue,
    }
}

struct SequenceExpansion {
    tokens: Vec<PpToken>,
    trailing_function_macro_can_continue: bool,
}

struct Expander<'a, 'location> {
    sources: &'a mut SourceMap,
    table: &'a mut MacroTable,
    options: &'a PreprocessOptions,
    location: ExpansionLocation<'location>,
    diagnostics: Vec<PpDiagnostic>,
    emitted_tokens: usize,
}

impl Expander<'_, '_> {
    fn expand_sequence(&mut self, tokens: &[PpToken]) -> SequenceExpansion {
        // Replacement tokens are spliced into the unconsumed input instead of
        // expanded in isolation.  Besides matching phase-4 rescanning, this is
        // what lets an object-like replacement become a function-like macro
        // invocation with a following `(` from the caller's token stream.
        let mut stream = tokens.to_vec();
        let mut output = Vec::new();
        let mut trailing_function_macro_can_continue = false;
        let mut index = 0;
        while index < stream.len() {
            if self.emitted_tokens >= self.options.limits.output_tokens {
                self.error(
                    &stream[index],
                    "CCC1102",
                    "preprocessing token limit exceeded",
                );
                break;
            }
            let token = stream[index].clone();
            if token.kind != PpTokenKind::Identifier {
                output.push(token);
                trailing_function_macro_can_continue = false;
                self.emitted_tokens += 1;
                index += 1;
                continue;
            }

            let name = token.identifier_key();
            if token.hide_set.contains(&name) {
                output.push(token);
                trailing_function_macro_can_continue = false;
                self.emitted_tokens += 1;
                index += 1;
                continue;
            }
            if token.expansion_depth > self.options.limits.expansion_depth {
                self.error(&token, "CCC1101", "macro expansion depth limit exceeded");
                output.push(token);
                trailing_function_macro_can_continue = false;
                self.emitted_tokens += 1;
                index += 1;
                continue;
            }
            if let Some(dynamic) = self.expand_dynamic(&token, &name) {
                output.push(dynamic);
                trailing_function_macro_can_continue = false;
                self.emitted_tokens += 1;
                index += 1;
                continue;
            }
            let Some(definition) = self.table.get(&name).cloned() else {
                output.push(token);
                trailing_function_macro_can_continue = false;
                self.emitted_tokens += 1;
                index += 1;
                continue;
            };

            match &definition.form {
                MacroForm::Object => {
                    let origin = self.sources.intern_origin(
                        OriginKind::MacroExpansion {
                            macro_name: definition.name.clone(),
                            invocation: token.span,
                            definition: definition.definition_span,
                        },
                        token.span.origin,
                        token.span,
                    );
                    let mut hide_set = token.hide_set.clone();
                    hide_set.insert(name);
                    let mut replacement =
                        invocation_tokens(&definition.replacement, &token, origin);
                    suppress_expansion(
                        &mut replacement,
                        &hide_set,
                        token.expansion_depth.saturating_add(1),
                    );
                    stream.splice(index..index + 1, replacement);
                }
                MacroForm::Function {
                    parameters,
                    variadic,
                } => {
                    if stream.get(index + 1).map(|next| next.spelling.as_str()) != Some("(") {
                        output.push(token);
                        trailing_function_macro_can_continue = index + 1 == stream.len();
                        self.emitted_tokens += 1;
                        index += 1;
                        continue;
                    }
                    let Some((arguments, end, variadic_was_absent)) =
                        self.parse_arguments(&stream, index + 1, parameters.len(), *variadic)
                    else {
                        output.push(token);
                        trailing_function_macro_can_continue = false;
                        self.emitted_tokens += 1;
                        index += 1;
                        continue;
                    };
                    let closing = &stream[end - 1];
                    let mut hide_set = token
                        .hide_set
                        .intersection(&closing.hide_set)
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    hide_set.insert(name);
                    let expansion_depth = token
                        .expansion_depth
                        .max(closing.expansion_depth)
                        .saturating_add(1);
                    let replacement = self.substitute(
                        &definition,
                        parameters,
                        *variadic,
                        &arguments,
                        variadic_was_absent,
                        &token,
                    );
                    let mut replacement = replacement;
                    suppress_expansion(&mut replacement, &hide_set, expansion_depth);
                    stream.splice(index..end, replacement);
                }
            }
        }
        SequenceExpansion {
            tokens: output,
            trailing_function_macro_can_continue,
        }
    }

    fn expand_dynamic(&mut self, token: &PpToken, name: &str) -> Option<PpToken> {
        if !self.table.dynamic_enabled(name) {
            return None;
        }
        let (kind, spelling) = match name {
            "__LINE__" => (PpTokenKind::PpNumber, token.logical_line.to_string()),
            "__FILE__" => (
                PpTokenKind::StringLiteral,
                quote_string(self.location.logical_file),
            ),
            "__DATE__" => (PpTokenKind::StringLiteral, self.options.date_macro.clone()),
            "__TIME__" => (PpTokenKind::StringLiteral, self.options.time_macro.clone()),
            "__COUNTER__" => (PpTokenKind::PpNumber, self.table.next_counter().to_string()),
            _ => return None,
        };
        let mut result = PpToken::synthetic(kind, token.span, spelling);
        inherit_invocation_state(&mut result, token);
        result.is_system_header = self.location.is_system_header;
        Some(result)
    }

    fn parse_arguments(
        &mut self,
        tokens: &[PpToken],
        open: usize,
        parameter_count: usize,
        variadic: bool,
    ) -> Option<(Vec<Vec<PpToken>>, usize, bool)> {
        let mut arguments = vec![Vec::new()];
        let mut depth = 1_usize;
        let mut index = open + 1;
        while index < tokens.len() {
            match tokens[index].spelling.as_str() {
                "(" => {
                    depth += 1;
                    if depth > self.options.limits.argument_depth {
                        self.error(
                            &tokens[index],
                            "CCC1103",
                            "macro argument nesting limit exceeded",
                        );
                        return None;
                    }
                    arguments.last_mut().unwrap().push(tokens[index].clone());
                }
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        let syntactically_empty = arguments.len() == 1 && arguments[0].is_empty();
                        if syntactically_empty && parameter_count == 0 {
                            arguments.clear();
                        }
                        let minimum = parameter_count;
                        let valid = if variadic {
                            arguments.len() >= minimum
                        } else {
                            arguments.len() == minimum
                        };
                        if !valid {
                            self.error(
                                &tokens[open],
                                "CCC1104",
                                format!(
                                    "macro expects {} argument{}, but {} {} provided",
                                    parameter_count,
                                    if parameter_count == 1 { "" } else { "s" },
                                    arguments.len(),
                                    if arguments.len() == 1 { "was" } else { "were" }
                                ),
                            );
                            return None;
                        }
                        let variadic_was_absent = variadic && arguments.len() == parameter_count;
                        return Some((arguments, index + 1, variadic_was_absent));
                    }
                    arguments.last_mut().unwrap().push(tokens[index].clone());
                }
                "," if depth == 1 => arguments.push(Vec::new()),
                _ => arguments.last_mut().unwrap().push(tokens[index].clone()),
            }
            index += 1;
        }
        self.error(&tokens[open], "CCC1105", "unterminated macro invocation");
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn substitute(
        &mut self,
        definition: &MacroDefinition,
        parameters: &[String],
        variadic: bool,
        arguments: &[Vec<PpToken>],
        variadic_was_absent: bool,
        invocation: &PpToken,
    ) -> Vec<PpToken> {
        let fixed_count = parameters.len();
        let raw_variadic = if variadic && arguments.len() > fixed_count {
            join_arguments(&arguments[fixed_count..], invocation)
        } else {
            Vec::new()
        };
        // Arguments are prescanned only when their parameter is substituted
        // outside `#`/`##`.  Cache that prescan so repeated parameters expand
        // dynamic macros such as `__COUNTER__` exactly once, while unused,
        // stringized, and pasted arguments are never expanded as a side effect.
        let mut expanded_arguments = vec![None::<Vec<PpToken>>; arguments.len()];
        let mut expanded_variadic = None::<Vec<PpToken>>;

        let replacement = &definition.replacement;
        let macro_origin = self.sources.intern_origin(
            OriginKind::MacroExpansion {
                macro_name: definition.name.clone(),
                invocation: invocation.span,
                definition: definition.definition_span,
            },
            invocation.span.origin,
            invocation.span,
        );
        let mut output = Vec::<PpToken>::new();
        let mut pending_paste = None;
        let mut index = 0;
        while index < replacement.len() {
            let token = &replacement[index];
            if is_hash(token) {
                let Some(parameter_token) = replacement.get(index + 1) else {
                    self.error(token, "CCC1106", "'#' must precede a macro parameter");
                    index += 1;
                    continue;
                };
                if let Some(argument) = raw_argument(
                    parameter_token,
                    parameters,
                    arguments,
                    variadic,
                    &raw_variadic,
                ) {
                    let mut string = PpToken::synthetic(
                        PpTokenKind::StringLiteral,
                        invocation.span,
                        stringize(argument),
                    );
                    let argument_span =
                        argument.first().map_or(invocation.span, |token| token.span);
                    let origin = self.sources.intern_origin(
                        OriginKind::Stringization {
                            operator: token.span,
                            argument: argument_span,
                        },
                        macro_origin,
                        invocation.span,
                    );
                    string.span = string.span.with_origin_id(origin);
                    string.leading_space = token.leading_space;
                    self.append_piece(&mut output, vec![string], &mut pending_paste, invocation);
                    index += 2;
                    continue;
                }
                self.error(token, "CCC1106", "'#' must precede a macro parameter");
                index += 1;
                continue;
            }
            if is_paste(token) {
                if pending_paste.is_some() || index + 1 == replacement.len() {
                    self.error(token, "CCC1107", "'##' cannot appear at this position");
                }
                pending_paste = Some(token.span);
                index += 1;
                continue;
            }

            let adjacent_to_paste = replacement.get(index.wrapping_sub(1)).is_some_and(is_paste)
                || replacement.get(index + 1).is_some_and(is_paste);
            let piece = if let Some(parameter_index) = parameter_index(token, parameters) {
                let mut piece = if adjacent_to_paste {
                    arguments.get(parameter_index).cloned().unwrap_or_default()
                } else {
                    if expanded_arguments[parameter_index].is_none() {
                        expanded_arguments[parameter_index] = Some(
                            self.expand_sequence(
                                arguments.get(parameter_index).map_or(&[], Vec::as_slice),
                            )
                            .tokens,
                        );
                    }
                    expanded_arguments[parameter_index]
                        .clone()
                        .unwrap_or_default()
                };
                self.mark_argument_origins(
                    &mut piece,
                    &parameters[parameter_index],
                    arguments
                        .get(parameter_index)
                        .and_then(|argument| argument.first()),
                    token.span,
                    invocation.span,
                );
                if let Some(first) = piece.first_mut() {
                    first.leading_space = token.leading_space;
                }
                piece
            } else if variadic && token.identifier_key() == "__VA_ARGS__" {
                let mut piece = if adjacent_to_paste {
                    raw_variadic.clone()
                } else {
                    if expanded_variadic.is_none() {
                        let mut expanded = Vec::with_capacity(arguments.len() - fixed_count);
                        for argument_index in fixed_count..arguments.len() {
                            if expanded_arguments[argument_index].is_none() {
                                expanded_arguments[argument_index] =
                                    Some(self.expand_sequence(&arguments[argument_index]).tokens);
                            }
                            expanded.push(
                                expanded_arguments[argument_index]
                                    .clone()
                                    .unwrap_or_default(),
                            );
                        }
                        expanded_variadic = Some(join_arguments(&expanded, invocation));
                    }
                    expanded_variadic.clone().unwrap_or_default()
                };
                self.mark_argument_origins(
                    &mut piece,
                    "__VA_ARGS__",
                    raw_variadic.first(),
                    token.span,
                    invocation.span,
                );
                if let Some(first) = piece.first_mut() {
                    first.leading_space = token.leading_space;
                }
                piece
            } else {
                let mut token = token.clone();
                token.span = token.span.with_origin_id(macro_origin);
                vec![token]
            };

            if pending_paste.is_some()
                && self.options.gnu_comma_elision
                && token.kind == PpTokenKind::Identifier
                && token.identifier_key() == "__VA_ARGS__"
                && output.last().is_some_and(|last| last.spelling == ",")
            {
                if variadic_was_absent {
                    output.pop();
                } else {
                    output.extend(piece);
                }
                pending_paste = None;
            } else {
                self.append_piece(&mut output, piece, &mut pending_paste, invocation);
            }
            index += 1;
        }
        if pending_paste.is_some() {
            self.error(invocation, "CCC1107", "'##' has no right operand");
        }
        if let Some(first) = output.first_mut() {
            first.leading_space = invocation.leading_space;
            first.at_start_of_line = invocation.at_start_of_line;
        }
        for token in &mut output {
            token.span = invocation.span.with_origin_id(token.span.origin);
            token.logical_line = invocation.logical_line;
            token.is_system_header = invocation.is_system_header;
        }
        output
    }

    fn append_piece(
        &mut self,
        output: &mut Vec<PpToken>,
        mut piece: Vec<PpToken>,
        pending_paste: &mut Option<Span>,
        invocation: &PpToken,
    ) {
        if pending_paste.is_none() {
            output.append(&mut piece);
            return;
        }
        let operator = pending_paste.take().expect("paste was pending");
        match (output.pop(), piece.is_empty()) {
            (None, _) => output.append(&mut piece),
            (Some(left), true) => output.push(left),
            (Some(left), false) => {
                let right = piece.remove(0);
                let spelling = format!("{}{}", left.spelling, right.spelling);
                if let Some(mut pasted) = relex_one(invocation, &spelling) {
                    pasted.leading_space = left.leading_space;
                    pasted.hide_set = left
                        .hide_set
                        .intersection(&right.hide_set)
                        .cloned()
                        .collect();
                    pasted.expansion_depth = left.expansion_depth.max(right.expansion_depth);
                    let origin = self.sources.intern_origin(
                        OriginKind::TokenPaste {
                            operator,
                            left: left.span,
                            right: right.span,
                        },
                        left.span.origin,
                        invocation.span,
                    );
                    pasted.span = pasted.span.with_origin_id(origin);
                    output.push(pasted);
                    output.append(&mut piece);
                } else {
                    self.error(
                        invocation,
                        "CCC1108",
                        format!(
                            "pasting '{}' and '{}' does not form a preprocessing token",
                            left.spelling, right.spelling
                        ),
                    );
                    output.push(left);
                    output.push(right);
                    output.append(&mut piece);
                }
            }
        }
    }

    fn mark_argument_origins(
        &mut self,
        piece: &mut [PpToken],
        parameter: &str,
        argument: Option<&PpToken>,
        replacement: Span,
        invocation: Span,
    ) {
        for token in piece {
            let origin = self.sources.intern_origin(
                OriginKind::ArgumentSubstitution {
                    parameter: parameter.to_owned(),
                    argument: argument.map_or(invocation, |argument| argument.span),
                    replacement,
                },
                token.span.origin,
                invocation,
            );
            token.span = invocation.with_origin_id(origin);
        }
    }

    fn error(&mut self, token: &PpToken, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(
            PpDiagnostic::new(PpSeverity::Error, code, message)
                .with_span(token.span)
                .in_system_header(token.is_system_header),
        );
    }
}

fn is_dynamic_name(name: &str) -> bool {
    matches!(
        name,
        "__LINE__"
            | "__FILE__"
            | "__DATE__"
            | "__TIME__"
            | "__COUNTER__"
            | "__has_include"
            | "__has_include_next"
            | "__has_attribute"
            | "__has_builtin"
            | "__has_feature"
            | "__has_extension"
    )
}

fn invocation_tokens(
    replacement: &[PpToken],
    invocation: &PpToken,
    origin: ccc_session::OriginId,
) -> Vec<PpToken> {
    let mut replacement = replacement.to_vec();
    for token in &mut replacement {
        token.span = invocation.span.with_origin_id(origin);
        token.logical_line = invocation.logical_line;
        token.is_system_header = invocation.is_system_header;
    }
    if let Some(first) = replacement.first_mut() {
        first.leading_space = invocation.leading_space;
        first.at_start_of_line = invocation.at_start_of_line;
    }
    replacement
}

fn suppress_expansion(tokens: &mut [PpToken], hide_set: &BTreeSet<String>, depth: usize) {
    for token in tokens {
        token.hide_set.extend(hide_set.iter().cloned());
        token.expansion_depth = token.expansion_depth.max(depth);
    }
}

fn inherit_invocation_state(token: &mut PpToken, invocation: &PpToken) {
    token.span = invocation.span.with_origin_id(token.span.origin);
    token.leading_space = invocation.leading_space;
    token.at_start_of_line = invocation.at_start_of_line;
    token.logical_line = invocation.logical_line;
    token.is_system_header = invocation.is_system_header;
    token.hide_set.clone_from(&invocation.hide_set);
    token.expansion_depth = invocation.expansion_depth.saturating_add(1);
}

fn is_hash(token: &PpToken) -> bool {
    matches!(token.spelling.as_str(), "#" | "%:")
}

fn is_paste(token: &PpToken) -> bool {
    matches!(token.spelling.as_str(), "##" | "%:%:")
}

fn parameter_index(token: &PpToken, parameters: &[String]) -> Option<usize> {
    if token.kind != PpTokenKind::Identifier {
        return None;
    }
    let key = token.identifier_key();
    parameters.iter().position(|parameter| *parameter == key)
}

fn raw_argument<'a>(
    token: &PpToken,
    parameters: &[String],
    arguments: &'a [Vec<PpToken>],
    variadic: bool,
    variadic_argument: &'a [PpToken],
) -> Option<&'a [PpToken]> {
    if let Some(index) = parameter_index(token, parameters) {
        return Some(arguments.get(index).map_or(&[], Vec::as_slice));
    }
    (variadic && token.kind == PpTokenKind::Identifier && token.identifier_key() == "__VA_ARGS__")
        .then_some(variadic_argument)
}

fn join_arguments(arguments: &[Vec<PpToken>], reference: &PpToken) -> Vec<PpToken> {
    let mut output = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index > 0 {
            let mut comma = PpToken::synthetic(PpTokenKind::Punctuator, reference.span, ",");
            comma.leading_space = false;
            output.push(comma);
        }
        output.extend(argument.iter().cloned());
    }
    output
}

fn stringize(tokens: &[PpToken]) -> String {
    let mut output = String::from("\"");
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 && token.leading_space {
            output.push(' ');
        }
        for character in token.spelling.chars() {
            if matches!(
                token.kind,
                PpTokenKind::StringLiteral | PpTokenKind::CharacterConstant
            ) && matches!(character, '\\' | '"')
            {
                output.push('\\');
            }
            output.push(character);
        }
    }
    output.push('"');
    output
}

fn quote_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

pub(crate) type ParsedPragmaOperators =
    (Vec<PpToken>, Vec<(usize, String, Span)>, Vec<PpDiagnostic>);

pub(crate) fn parse_pragma_operators(tokens: Vec<PpToken>) -> ParsedPragmaOperators {
    let mut output = Vec::new();
    let mut pragmas = Vec::new();
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].kind == PpTokenKind::Identifier
            && tokens[index].identifier_key() == "_Pragma"
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.spelling == "(")
        {
            let valid = tokens
                .get(index + 2)
                .filter(|token| token.kind == PpTokenKind::StringLiteral)
                .zip(tokens.get(index + 3))
                .filter(|(_, close)| close.spelling == ")");
            if let Some((literal, _)) = valid {
                match unquote_pragma(&literal.spelling) {
                    Ok(text) => pragmas.push((output.len(), text, tokens[index].span)),
                    Err(message) => diagnostics
                        .push(PpDiagnostic::error("CCC1109", message).with_span(literal.span)),
                }
                index += 4;
                continue;
            }
            diagnostics.push(
                PpDiagnostic::error("CCC1109", "_Pragma requires one string literal operand")
                    .with_span(tokens[index].span),
            );
        }
        output.push(tokens[index].clone());
        index += 1;
    }
    (output, pragmas, diagnostics)
}

fn unquote_pragma(spelling: &str) -> Result<String, &'static str> {
    let body = spelling
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
        .ok_or("invalid _Pragma string literal")?;
    let mut output = String::new();
    let mut chars = body.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => return Err("incomplete escape in _Pragma string"),
        }
    }
    Ok(output)
}

pub(crate) fn canonical_macro_name(name: &str) -> Option<String> {
    canonicalize_identifier(name)
}

pub(crate) fn redefinition_diagnostic(
    definition: &MacroDefinition,
    previous: &MacroDefinition,
) -> PpDiagnostic {
    PpDiagnostic::new(
        PpSeverity::Warning,
        "CCC1110",
        format!("macro '{}' redefined", definition.name),
    )
    .with_span(definition.definition_span)
    .with_category(PpDiagnosticCategory::MacroRedefined)
    .with_secondary(previous.definition_span, "previous definition")
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

    use super::*;
    use crate::lexer::lex;

    fn tokens(sources: &mut SourceMap, source: &str) -> (ccc_session::FileId, Vec<PpToken>) {
        let file = sources.add_file("test.c", source);
        (file, lex(file, source).unwrap())
    }

    #[test]
    fn macro_equivalence_ignores_whitespace_before_the_replacement_list() {
        let mut sources = SourceMap::new();
        let (file, compact) = tokens(&mut sources, "a + b");
        let (_, indented) = tokens(&mut sources, " a + b");
        let (_, changed_internal_spacing) = tokens(&mut sources, "a+ b");
        let definition = |replacement| MacroDefinition {
            name: "F".to_owned(),
            form: MacroForm::Function {
                parameters: vec!["a".to_owned(), "b".to_owned()],
                variadic: false,
            },
            replacement,
            definition_span: Span::new(file, 0, 1),
            predefined: false,
        };

        let mut table = MacroTable::default();
        assert!(matches!(
            table.define(definition(compact)),
            DefineResult::Inserted
        ));
        assert!(matches!(
            table.define(definition(indented)),
            DefineResult::Equivalent
        ));
        assert!(matches!(
            table.define(definition(changed_internal_spacing)),
            DefineResult::Replaced(_)
        ));
    }

    #[test]
    fn expands_object_function_stringize_and_paste() {
        let mut sources = SourceMap::new();
        let (file, replacement) = tokens(&mut sources, "x ## y # x");
        let mut table = MacroTable::default();
        table.define(MacroDefinition {
            name: "F".to_owned(),
            form: MacroForm::Function {
                parameters: vec!["x".to_owned(), "y".to_owned()],
                variadic: false,
            },
            replacement,
            definition_span: Span::new(file, 0, 1),
            predefined: false,
        });
        let (_, invocation) = tokens(&mut sources, "F(hel, lo)");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        let spellings: Vec<_> = result
            .tokens
            .iter()
            .map(|token| token.spelling.as_str())
            .collect();
        assert_eq!(spellings, ["hello", "\"hel\""]);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn stringizes_catch_all_backslashes_and_literal_delimiters() {
        let mut sources = SourceMap::new();
        let (_, catch_all) = tokens(&mut sources, r"a\b");
        assert_eq!(stringize(&catch_all), r#""a\b""#);

        let (_, literal) = tokens(&mut sources, r#""text""#);
        assert_eq!(stringize(&literal), r#""\"text\"""#);
    }

    #[test]
    fn suppresses_recursive_expansion() {
        let mut sources = SourceMap::new();
        let (file, a_replacement) = tokens(&mut sources, "b");
        let (_, b_replacement) = tokens(&mut sources, "a");
        let mut table = MacroTable::default();
        for (name, replacement) in [("a", a_replacement), ("b", b_replacement)] {
            table.define(MacroDefinition {
                name: name.to_owned(),
                form: MacroForm::Object,
                replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            });
        }
        let (_, invocation) = tokens(&mut sources, "a");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        assert_eq!(result.tokens[0].spelling, "a");
    }

    #[test]
    fn retains_argument_hide_sets_during_nested_rescanning() {
        let mut sources = SourceMap::new();
        let (file, x_replacement) = tokens(&mut sources, "2");
        let (_, f_replacement) = tokens(&mut sources, "f(x * (a))");
        let (_, z_replacement) = tokens(&mut sources, "z[0]");
        let mut table = MacroTable::default();
        for definition in [
            MacroDefinition {
                name: "x".to_owned(),
                form: MacroForm::Object,
                replacement: x_replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            },
            MacroDefinition {
                name: "f".to_owned(),
                form: MacroForm::Function {
                    parameters: vec!["a".to_owned()],
                    variadic: false,
                },
                replacement: f_replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            },
            MacroDefinition {
                name: "z".to_owned(),
                form: MacroForm::Object,
                replacement: z_replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            },
        ] {
            table.define(definition);
        }
        let (_, invocation) = tokens(&mut sources, "f(f(z))");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        let (_, expected) = tokens(&mut sources, "f(2 * (f(2 * (z[0]))))");

        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| (&token.kind, token.spelling.as_str()))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|token| (&token.kind, token.spelling.as_str()))
                .collect::<Vec<_>>()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn suppresses_indirect_recursion_across_a_following_invocation() {
        let mut sources = SourceMap::new();
        let (file, a_replacement) = tokens(&mut sources, "B");
        let (_, b_replacement) = tokens(&mut sources, "A");
        let mut table = MacroTable::default();
        for (name, replacement) in [("A", a_replacement), ("B", b_replacement)] {
            table.define(MacroDefinition {
                name: name.to_owned(),
                form: MacroForm::Function {
                    parameters: Vec::new(),
                    variadic: false,
                },
                replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            });
        }
        let (_, invocation) = tokens(&mut sources, "A()()");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );

        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["A"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn intersects_function_invocation_hide_sets_at_the_closing_parenthesis() {
        let mut sources = SourceMap::new();
        let (file, f_replacement) = tokens(&mut sources, "a * g");
        let (_, g_replacement) = tokens(&mut sources, "f(a)");
        let mut table = MacroTable::default();
        for (name, replacement) in [("f", f_replacement), ("g", g_replacement)] {
            table.define(MacroDefinition {
                name: name.to_owned(),
                form: MacroForm::Function {
                    parameters: vec!["a".to_owned()],
                    variadic: false,
                },
                replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            });
        }
        let (_, invocation) = tokens(&mut sources, "f(2)(9)");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );

        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["2", "*", "9", "*", "g"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn rescans_an_object_replacement_with_following_input_tokens() {
        let mut sources = SourceMap::new();
        let (file, f_replacement) = tokens(&mut sources, "x");
        let (_, g_replacement) = tokens(&mut sources, "f");
        let mut table = MacroTable::default();
        table.define(MacroDefinition {
            name: "f".to_owned(),
            form: MacroForm::Function {
                parameters: vec!["x".to_owned()],
                variadic: false,
            },
            replacement: f_replacement,
            definition_span: Span::new(file, 0, 1),
            predefined: false,
        });
        table.define(MacroDefinition {
            name: "g".to_owned(),
            form: MacroForm::Object,
            replacement: g_replacement,
            definition_span: Span::new(file, 0, 1),
            predefined: false,
        });
        let (_, invocation) = tokens(&mut sources, "g(2)");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );

        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["2"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn prescans_only_arguments_that_are_substituted_normally() {
        let mut sources = SourceMap::new();
        let (file, ignored_replacement) = tokens(&mut sources, "0");
        let (_, string_replacement) = tokens(&mut sources, "#x");
        let (_, paste_replacement) = tokens(&mut sources, "pre ## x");
        let (_, twice_replacement) = tokens(&mut sources, "x x");
        let mut table = MacroTable::default();
        for (name, replacement) in [
            ("IGNORE", ignored_replacement),
            ("STRING", string_replacement),
            ("PASTE", paste_replacement),
            ("TWICE", twice_replacement),
        ] {
            table.define(MacroDefinition {
                name: name.to_owned(),
                form: MacroForm::Function {
                    parameters: vec!["x".to_owned()],
                    variadic: false,
                },
                replacement,
                definition_span: Span::new(file, 0, 1),
                predefined: false,
            });
        }
        let (_, invocation) = tokens(
            &mut sources,
            concat!(
                "IGNORE(__COUNTER__) ",
                "STRING(__COUNTER__) ",
                "PASTE(__COUNTER__) ",
                "TWICE(__COUNTER__) ",
                "__COUNTER__",
            ),
        );
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );

        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["0", "\"__COUNTER__\"", "pre__COUNTER__", "0", "0", "1"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn treats_empty_paste_operands_as_placemarkers() {
        let mut sources = SourceMap::new();
        let (file, replacement) = tokens(&mut sources, "left ## right");
        let mut table = MacroTable::default();
        table.define(MacroDefinition {
            name: "CAT".to_owned(),
            form: MacroForm::Function {
                parameters: vec!["left".to_owned(), "right".to_owned()],
                variadic: false,
            },
            replacement,
            definition_span: Span::new(file, 0, 3),
            predefined: false,
        });
        let (_, invocation) = tokens(&mut sources, "CAT(,x) CAT(y,)");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn gnu_comma_elision_does_not_paste_a_present_variadic_argument() {
        let mut sources = SourceMap::new();
        let (file, replacement) = tokens(&mut sources, "fixed , ## __VA_ARGS__");
        let mut table = MacroTable::default();
        table.define(MacroDefinition {
            name: "ARGS".to_owned(),
            form: MacroForm::Function {
                parameters: vec!["fixed".to_owned()],
                variadic: true,
            },
            replacement,
            definition_span: Span::new(file, 0, 4),
            predefined: false,
        });
        let (_, invocation) = tokens(&mut sources, "ARGS(1) ARGS(1, 2)");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        assert_eq!(
            result
                .tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["1", "1", ",", "2"]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn retains_macro_origin_on_the_first_replacement_token() {
        let mut sources = SourceMap::new();
        let (file, replacement) = tokens(&mut sources, "42");
        let mut table = MacroTable::default();
        table.define(MacroDefinition {
            name: "ANSWER".to_owned(),
            form: MacroForm::Object,
            replacement,
            definition_span: Span::new(file, 0, 6),
            predefined: false,
        });
        let (_, invocation) = tokens(&mut sources, "ANSWER");
        let result = expand(
            &mut sources,
            &mut table,
            &invocation,
            &PreprocessOptions::default(),
            ExpansionLocation {
                logical_file: "test.c",
                is_system_header: false,
            },
        );
        assert!(!result.tokens[0].span.origin.is_direct());
        assert!(matches!(
            sources.origin(result.tokens[0].span.origin).map(|origin| &origin.kind),
            Some(OriginKind::MacroExpansion { macro_name, .. }) if macro_name == "ANSWER"
        ));
    }
}
