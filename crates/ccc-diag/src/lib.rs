//! Structured diagnostics, warning policy, and deterministic text rendering.

mod code;

use std::collections::HashMap;
use std::fmt;

use ccc_session::{OriginKind, SourceLocation, SourceMap, Span};

pub use code::{
    ALL, DiagnosticCode, DiagnosticCodeDefinition, DiagnosticOwner, DiagnosticOwnerBand,
    OWNER_BANDS, codes,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticFormat {
    #[default]
    Text,
    Json,
}

impl Severity {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
        }
    }
}

/// A warning group such as `unused-macros`, without the `-W` prefix.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WarningCategory(String);

impl WarningCategory {
    pub fn new(category: impl Into<String>) -> Self {
        let category = category.into();
        Self(category.strip_prefix("-W").unwrap_or(&category).to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: Into<String>> From<T> for WarningCategory {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for WarningCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub category: Option<Box<WarningCategory>>,
    pub primary: Option<Box<PrimarySpan>>,
    pub secondary: Vec<SecondarySpan>,
    pub notes: Vec<String>,
    warning_option: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimarySpan {
    pub span: Span,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecondarySpan {
    pub span: Span,
    pub label: Option<String>,
}

/// Limits used while rendering recursive source context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    pub include_trace_limit: usize,
    pub macro_backtrace_limit: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            include_trace_limit: 32,
            macro_backtrace_limit: 8,
        }
    }
}

impl Diagnostic {
    pub fn new(severity: Severity, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: code.into(),
            message: message.into(),
            category: None,
            primary: None,
            secondary: Vec::new(),
            notes: Vec::new(),
            warning_option: false,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    pub fn warning(
        code: impl Into<String>,
        category: impl Into<WarningCategory>,
        message: impl Into<String>,
    ) -> Self {
        let mut diagnostic = Self::new(Severity::Warning, code, message).with_category(category);
        diagnostic.warning_option = true;
        diagnostic
    }

    pub fn with_category(mut self, category: impl Into<WarningCategory>) -> Self {
        self.category = Some(Box::new(category.into()));
        self
    }

    pub fn with_primary(mut self, span: Span, label: impl Into<String>) -> Self {
        self.primary = Some(Box::new(PrimarySpan {
            span,
            label: Some(label.into()),
        }));
        self
    }

    pub fn with_primary_span(mut self, span: Span) -> Self {
        self.primary = Some(Box::new(PrimarySpan { span, label: None }));
        self
    }

    pub fn with_secondary(mut self, span: Span, label: impl Into<String>) -> Self {
        self.secondary.push(SecondarySpan {
            span,
            label: Some(label.into()),
        });
        self
    }

    pub fn with_secondary_span(mut self, span: Span) -> Self {
        self.secondary.push(SecondarySpan { span, label: None });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Renders source annotations, include ancestry, and macro provenance.
    pub fn render(&self, sources: &SourceMap) -> String {
        self.render_with_options(sources, RenderOptions::default())
    }

    pub fn render_with_options(&self, sources: &SourceMap, options: RenderOptions) -> String {
        let mut rendered = render_header(self);

        if let Some(primary) = &self.primary {
            render_include_trace(
                &mut rendered,
                sources,
                primary.span,
                options.include_trace_limit,
            );
            render_span_annotation(
                &mut rendered,
                sources,
                primary.span,
                primary.label.as_deref(),
                "-->",
            );
        }

        for secondary in &self.secondary {
            render_span_annotation(
                &mut rendered,
                sources,
                secondary.span,
                secondary.label.as_deref(),
                ":::",
            );
        }

        for note in &self.notes {
            rendered.push_str("note: ");
            rendered.push_str(note);
            rendered.push('\n');
        }

        if let Some(primary) = &self.primary {
            render_macro_trace(
                &mut rendered,
                sources,
                primary.span,
                options.macro_backtrace_limit,
            );
        }

        rendered
    }
}

fn render_header(diagnostic: &Diagnostic) -> String {
    let category = if diagnostic.warning_option {
        diagnostic
            .category
            .as_ref()
            .map(|category| format!(" [-W{category}]"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "{}[{}]{category}: {}\n",
        diagnostic.severity.as_str(),
        diagnostic.code,
        diagnostic.message
    )
}

fn render_include_trace(rendered: &mut String, sources: &SourceMap, span: Span, max_depth: usize) {
    let trace = sources.include_trace(span.file, max_depth);
    for site in trace.sites.iter().rev() {
        let Some(location) = sources.presumed_location(site.directive.file, site.directive.start)
        else {
            continue;
        };
        rendered.push_str(&format!(
            " included from {}:{}:{}\n",
            location.file_name, location.line, location.column
        ));
    }
    if trace.truncated {
        rendered.push_str(" included from ... (include trace truncated)\n");
    }
}

fn render_span_annotation(
    rendered: &mut String,
    sources: &SourceMap,
    span: Span,
    label: Option<&str>,
    arrow: &str,
) {
    let Some(location) = sources.presumed_location(span.file, span.start) else {
        return;
    };
    let Some(physical) = sources.location(span.file, span.start) else {
        return;
    };
    let Some(source_line) = sources.line_text(span.file, physical.line) else {
        return;
    };

    let gutter_width = location.line.to_string().len();
    rendered.push_str(&format!(
        " {arrow} {}:{}:{}\n",
        location.file_name, location.line, location.column
    ));
    rendered.push_str(&format!(" {:>gutter_width$} |\n", ""));
    rendered.push_str(&format!(
        " {:>gutter_width$} | {source_line}\n",
        location.line
    ));
    rendered.push_str(&format!(" {:>gutter_width$} | ", ""));
    rendered.push_str(&caret_padding(source_line, physical.column));
    rendered.push_str(&"^".repeat(caret_width(sources, span, physical)));
    if let Some(label) = label {
        rendered.push(' ');
        rendered.push_str(label);
    }
    rendered.push('\n');
}

fn render_macro_trace(rendered: &mut String, sources: &SourceMap, span: Span, max_depth: usize) {
    let trace = sources.origin_trace(span.origin, max_depth);
    for origin in trace.frames {
        match &origin.kind {
            OriginKind::MacroExpansion {
                macro_name,
                invocation,
                definition,
            } => {
                render_trace_site(
                    rendered,
                    sources,
                    *invocation,
                    &format!("in expansion of macro `{macro_name}`"),
                );
                render_trace_site(
                    rendered,
                    sources,
                    *definition,
                    &format!("macro `{macro_name}` defined here"),
                );
            }
            OriginKind::ArgumentSubstitution {
                parameter,
                argument,
                replacement,
            } => {
                render_trace_site(
                    rendered,
                    sources,
                    *argument,
                    &format!("argument substituted for `{parameter}`"),
                );
                render_trace_site(
                    rendered,
                    sources,
                    *replacement,
                    &format!("`{parameter}` used here"),
                );
            }
            OriginKind::Stringization { operator, argument } => {
                render_trace_site(rendered, sources, *operator, "stringized here");
                render_trace_site(rendered, sources, *argument, "stringized argument");
            }
            OriginKind::TokenPaste {
                operator,
                left,
                right,
            } => {
                render_trace_site(rendered, sources, *operator, "tokens pasted here");
                render_trace_site(rendered, sources, *left, "left paste operand");
                render_trace_site(rendered, sources, *right, "right paste operand");
            }
        }
    }
    if trace.truncated {
        rendered.push_str("note: macro backtrace truncated\n");
    }
}

fn render_trace_site(rendered: &mut String, sources: &SourceMap, span: Span, message: &str) {
    let Some(location) = sources.presumed_location(span.file, span.start) else {
        return;
    };
    rendered.push_str(&format!(
        "note: {message}\n ::: {}:{}:{}\n",
        location.file_name, location.line, location.column
    ));
}

fn caret_padding(source_line: &str, column: usize) -> String {
    source_line
        .chars()
        .take(column.saturating_sub(1))
        .map(|character| if character == '\t' { '\t' } else { ' ' })
        .collect()
}

fn caret_width(sources: &SourceMap, span: Span, start: SourceLocation) -> usize {
    let Some(end) = sources.location(span.file, span.end) else {
        return 1;
    };
    if end.line == start.line {
        return end.column.saturating_sub(start.column).max(1);
    }

    sources.line_text(span.file, start.line).map_or(1, |line| {
        line.chars()
            .count()
            .saturating_add(1)
            .saturating_sub(start.column)
            .max(1)
    })
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(render_header(self).trim_end())
    }
}

/// The effective treatment of a warning category.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WarningLevel {
    #[default]
    Default,
    Ignored,
    Warning,
    Error,
}

/// Compilation-wide diagnostic policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticOptions {
    pub warnings_enabled: bool,
    pub warnings_as_errors: bool,
    pub suppress_warnings_in_system_headers: bool,
    /// Zero disables the error limit.
    pub error_limit: usize,
    pub render: RenderOptions,
}

impl Default for DiagnosticOptions {
    fn default() -> Self {
        Self {
            warnings_enabled: true,
            warnings_as_errors: false,
            suppress_warnings_in_system_headers: true,
            error_limit: 20,
            render: RenderOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmitOutcome {
    Emitted,
    Suppressed,
    Halted,
}

#[derive(Clone, Debug, Default)]
struct WarningState {
    levels: HashMap<WarningCategory, WarningLevel>,
}

/// Collects diagnostics and applies warning and error-limit policy.
#[derive(Debug, Default)]
pub struct DiagnosticEngine {
    options: DiagnosticOptions,
    warning_state: WarningState,
    warning_stack: Vec<WarningState>,
    diagnostics: Vec<Diagnostic>,
    error_count: usize,
    halted: bool,
}

impl DiagnosticEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: DiagnosticOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    pub fn options(&self) -> &DiagnosticOptions {
        &self.options
    }

    pub fn options_mut(&mut self) -> &mut DiagnosticOptions {
        &mut self.options
    }

    pub fn set_warnings_as_errors(&mut self, enabled: bool) {
        self.options.warnings_as_errors = enabled;
    }

    pub fn set_warn_in_system_headers(&mut self, enabled: bool) {
        self.options.suppress_warnings_in_system_headers = !enabled;
    }

    pub fn set_warning_level(&mut self, category: impl Into<WarningCategory>, level: WarningLevel) {
        let category = category.into();
        if level == WarningLevel::Default {
            self.warning_state.levels.remove(&category);
        } else {
            self.warning_state.levels.insert(category, level);
        }
    }

    pub fn warning_level(&self, category: &WarningCategory) -> WarningLevel {
        self.warning_state
            .levels
            .get(category)
            .copied()
            .unwrap_or_default()
    }

    /// Saves warning-category state for a diagnostic pragma `push`.
    pub fn push_warning_state(&mut self) {
        self.warning_stack.push(self.warning_state.clone());
    }

    /// Restores warning-category state for a diagnostic pragma `pop`.
    pub fn pop_warning_state(&mut self) -> bool {
        let Some(state) = self.warning_stack.pop() else {
            return false;
        };
        self.warning_state = state;
        true
    }

    pub fn emit(&mut self, sources: &SourceMap, mut diagnostic: Diagnostic) -> EmitOutcome {
        if self.halted {
            return EmitOutcome::Halted;
        }

        if diagnostic.severity == Severity::Warning {
            if !self.options.warnings_enabled {
                return EmitOutcome::Suppressed;
            }

            let level = diagnostic
                .category
                .as_ref()
                .map(|category| self.warning_level(category))
                .unwrap_or_default();
            if level == WarningLevel::Ignored {
                return EmitOutcome::Suppressed;
            }
            if self.options.suppress_warnings_in_system_headers
                && diagnostic
                    .primary
                    .as_ref()
                    .is_some_and(|primary| sources.is_system_header(primary.span))
            {
                return EmitOutcome::Suppressed;
            }
            if level == WarningLevel::Error
                || (level != WarningLevel::Warning && self.options.warnings_as_errors)
            {
                diagnostic.severity = Severity::Error;
            }
        }

        if diagnostic.severity == Severity::Error {
            self.error_count += 1;
        }
        self.diagnostics.push(diagnostic);

        if self.options.error_limit != 0 && self.error_count >= self.options.error_limit {
            self.diagnostics.push(
                Diagnostic::error(
                    codes::diagnostics::TOO_MANY_ERRORS,
                    format!(
                        "too many errors emitted; stopping after {}",
                        self.options.error_limit
                    ),
                )
                .with_category("diagnostics"),
            );
            self.halted = true;
        }

        EmitOutcome::Emitted
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub const fn error_count(&self) -> usize {
        self.error_count
    }

    pub const fn has_errors(&self) -> bool {
        self.error_count != 0
    }

    pub const fn is_halted(&self) -> bool {
        self.halted
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render_with_options(sources, self.options.render))
            .collect()
    }

    pub fn render_format(&self, sources: &SourceMap, format: DiagnosticFormat) -> String {
        match format {
            DiagnosticFormat::Text => self.render(sources),
            DiagnosticFormat::Json => {
                render_json_document(&self.diagnostics, sources, self.options.render)
            }
        }
    }
}

/// Renders a complete, deterministic machine-readable diagnostic document.
/// Schema changes require incrementing `schema_version`.
pub fn render_json_document(
    diagnostics: &[Diagnostic],
    sources: &SourceMap,
    options: RenderOptions,
) -> String {
    let mut output = String::from("{\"schema_version\":1,\"diagnostics\":[");
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        render_json_diagnostic(&mut output, diagnostic, sources, options);
    }
    output.push_str("]}\n");
    output
}

fn render_json_diagnostic(
    output: &mut String,
    diagnostic: &Diagnostic,
    sources: &SourceMap,
    options: RenderOptions,
) {
    output.push('{');
    json_field_string(output, "severity", diagnostic.severity.as_str());
    output.push(',');
    json_field_string(output, "code", &diagnostic.code);
    output.push_str(",\"category\":");
    json_optional_string(
        output,
        diagnostic
            .category
            .as_ref()
            .map(|category| category.as_str()),
    );
    output.push(',');
    json_field_string(output, "message", &diagnostic.message);
    output.push_str(",\"primary\":");
    if let Some(primary) = &diagnostic.primary {
        render_json_annotation(output, sources, primary.span, primary.label.as_deref());
    } else {
        output.push_str("null");
    }
    output.push_str(",\"secondary\":[");
    for (index, secondary) in diagnostic.secondary.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        render_json_annotation(output, sources, secondary.span, secondary.label.as_deref());
    }
    output.push_str("],\"notes\":[");
    for (index, note) in diagnostic.notes.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        json_string(output, note);
    }
    output.push_str("],\"include_trace\":");
    if let Some(primary) = &diagnostic.primary {
        render_json_include_trace(output, sources, primary.span, options.include_trace_limit);
    } else {
        output.push_str("{\"truncated\":false,\"frames\":[]}");
    }
    output.push_str(",\"macro_trace\":");
    if let Some(primary) = &diagnostic.primary {
        render_json_macro_trace(output, sources, primary.span, options.macro_backtrace_limit);
    } else {
        output.push_str("{\"truncated\":false,\"frames\":[]}");
    }
    output.push('}');
}

fn render_json_annotation(
    output: &mut String,
    sources: &SourceMap,
    span: Span,
    label: Option<&str>,
) {
    output.push_str("{\"label\":");
    json_optional_string(output, label);
    output.push_str(",\"location\":");
    render_json_location(output, sources, span);
    output.push('}');
}

fn render_json_location(output: &mut String, sources: &SourceMap, span: Span) {
    let Some(spec) = sources.file_spec(span.file) else {
        output.push_str("null");
        return;
    };
    let start = sources.presumed_location(span.file, span.start);
    let end = sources.presumed_location(span.file, span.end);
    output.push('{');
    json_field_string(output, "spelled_path", &spec.spelled_path.to_string_lossy());
    output.push(',');
    json_field_string(
        output,
        "resolved_path",
        &spec.resolved_path.to_string_lossy(),
    );
    output.push(',');
    json_field_string(
        output,
        "display_path",
        start.map_or(spec.display_name.as_str(), |location| location.file_name),
    );
    output.push_str(",\"start\":");
    render_json_position(
        output,
        span.start,
        start.map(|location| (location.line, location.column)),
    );
    output.push_str(",\"end\":");
    render_json_position(
        output,
        span.end,
        end.map(|location| (location.line, location.column)),
    );
    output.push('}');
}

fn render_json_position(output: &mut String, byte: usize, position: Option<(usize, usize)>) {
    output.push_str("{\"byte\":");
    output.push_str(&byte.to_string());
    output.push_str(",\"line\":");
    match position {
        Some((line, column)) => {
            output.push_str(&line.to_string());
            output.push_str(",\"column\":");
            output.push_str(&column.to_string());
        }
        None => output.push_str("null,\"column\":null"),
    }
    output.push('}');
}

fn render_json_include_trace(
    output: &mut String,
    sources: &SourceMap,
    span: Span,
    max_depth: usize,
) {
    let trace = sources.include_trace(span.file, max_depth);
    output.push_str("{\"truncated\":");
    output.push_str(if trace.truncated { "true" } else { "false" });
    output.push_str(",\"frames\":[");
    for (index, site) in trace.sites.iter().rev().enumerate() {
        if index != 0 {
            output.push(',');
        }
        render_json_location(output, sources, site.directive);
    }
    output.push_str("]}");
}

fn render_json_macro_trace(output: &mut String, sources: &SourceMap, span: Span, max_depth: usize) {
    let trace = sources.origin_trace(span.origin, max_depth);
    output.push_str("{\"truncated\":");
    output.push_str(if trace.truncated { "true" } else { "false" });
    output.push_str(",\"frames\":[");
    for (index, origin) in trace.frames.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        match &origin.kind {
            OriginKind::MacroExpansion {
                macro_name,
                invocation,
                definition,
            } => {
                output.push('{');
                json_field_string(output, "kind", "macro_expansion");
                output.push(',');
                json_field_string(output, "name", macro_name);
                output.push_str(",\"invocation\":");
                render_json_location(output, sources, *invocation);
                output.push_str(",\"definition\":");
                render_json_location(output, sources, *definition);
                output.push('}');
            }
            OriginKind::ArgumentSubstitution {
                parameter,
                argument,
                replacement,
            } => {
                output.push('{');
                json_field_string(output, "kind", "argument_substitution");
                output.push(',');
                json_field_string(output, "name", parameter);
                output.push_str(",\"argument\":");
                render_json_location(output, sources, *argument);
                output.push_str(",\"replacement\":");
                render_json_location(output, sources, *replacement);
                output.push('}');
            }
            OriginKind::Stringization { operator, argument } => {
                output.push('{');
                json_field_string(output, "kind", "stringization");
                output.push_str(",\"operator\":");
                render_json_location(output, sources, *operator);
                output.push_str(",\"argument\":");
                render_json_location(output, sources, *argument);
                output.push('}');
            }
            OriginKind::TokenPaste {
                operator,
                left,
                right,
            } => {
                output.push('{');
                json_field_string(output, "kind", "token_paste");
                output.push_str(",\"operator\":");
                render_json_location(output, sources, *operator);
                output.push_str(",\"left\":");
                render_json_location(output, sources, *left);
                output.push_str(",\"right\":");
                render_json_location(output, sources, *right);
                output.push('}');
            }
        }
    }
    output.push_str("]}");
}

fn json_field_string(output: &mut String, name: &str, value: &str) {
    json_string(output, name);
    output.push(':');
    json_string(output, value);
}

fn json_optional_string(output: &mut String, value: Option<&str>) {
    match value {
        Some(value) => json_string(output, value),
        None => output.push_str("null"),
    }
}

fn json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use ccc_session::{OriginId, SourceFileSpec};

    use super::*;

    #[test]
    fn renders_a_primary_span_with_a_caret() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int ;\n");
        let diagnostic = Diagnostic::error("CCC0001", "unexpected token")
            .with_primary(Span::new(file, 4, 5), "expected an identifier");

        assert_eq!(
            diagnostic.render(&sources),
            concat!(
                "error[CCC0001]: unexpected token\n",
                " --> test.c:1:5\n",
                "   |\n",
                " 1 | int ;\n",
                "   |     ^ expected an identifier\n",
            )
        );
    }

    #[test]
    fn renders_secondary_spans_notes_and_presumed_locations() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("generated.c", "int first;\nint second;\n");
        sources
            .add_line_mapping(file, 2, 80, Some("original.c".into()))
            .unwrap();
        let diagnostic = Diagnostic::error("CCC1000", "conflicting declaration")
            .with_primary(Span::new(file, 15, 21), "conflicts here")
            .with_secondary(Span::new(file, 4, 9), "first declared here")
            .with_note("declarations must agree");

        let rendered = diagnostic.render(&sources);
        assert!(rendered.contains(" --> original.c:80:5\n"));
        assert!(rendered.contains(" ::: generated.c:1:5\n"));
        assert!(rendered.contains("note: declarations must agree\n"));
    }

    #[test]
    fn renders_include_and_bounded_macro_traces() {
        let mut sources = SourceMap::new();
        let main = sources.add_file("main.c", "#include \"value.h\"\n");
        let directive = Span::new(main, 0, 18);
        let header = sources.add_file_occurrence(
            SourceFileSpec::new("value.h").included_from(main, directive),
            "VALUE\n",
        );
        let token = Span::new(header, 0, 5);
        let origin = sources.intern_origin(
            OriginKind::MacroExpansion {
                macro_name: "VALUE".into(),
                invocation: token,
                definition: token,
            },
            OriginId::DIRECT,
            token,
        );
        let diagnostic = Diagnostic::error("CCC1001", "bad expansion")
            .with_primary_span(token.with_origin_id(origin));

        let rendered = diagnostic.render_with_options(
            &sources,
            RenderOptions {
                include_trace_limit: 4,
                macro_backtrace_limit: 1,
            },
        );
        assert!(rendered.contains(" included from main.c:1:1\n"));
        assert!(rendered.contains("note: in expansion of macro `VALUE`\n"));
        assert!(rendered.contains("note: macro `VALUE` defined here\n"));

        let json = render_json_document(
            &[diagnostic],
            &sources,
            RenderOptions {
                include_trace_limit: 0,
                macro_backtrace_limit: 0,
            },
        );
        assert_eq!(json.matches("\"truncated\":true").count(), 2, "{json}");
        assert!(json.contains("\"include_trace\":{\"truncated\":true,\"frames\":[]}"));
        assert!(json.contains("\"macro_trace\":{\"truncated\":true,\"frames\":[]}"));
    }

    #[test]
    fn applies_warning_controls_and_werror() {
        let sources = SourceMap::new();
        let mut engine = DiagnosticEngine::new();
        engine.set_warning_level("unused-macros", WarningLevel::Ignored);
        assert_eq!(
            engine.emit(
                &sources,
                Diagnostic::warning("CCC0100", "unused-macros", "unused macro")
            ),
            EmitOutcome::Suppressed
        );

        engine.push_warning_state();
        engine.set_warning_level("unused-macros", WarningLevel::Error);
        assert_eq!(
            engine.emit(
                &sources,
                Diagnostic::warning("CCC0100", "unused-macros", "unused macro")
            ),
            EmitOutcome::Emitted
        );
        assert!(engine.has_errors());
        assert_eq!(engine.diagnostics()[0].severity, Severity::Error);
        assert!(
            engine
                .render(&sources)
                .contains("error[CCC0100] [-Wunused-macros]")
        );
        assert!(engine.pop_warning_state());
        assert_eq!(
            engine.warning_level(&WarningCategory::new("unused-macros")),
            WarningLevel::Ignored
        );
    }

    #[test]
    fn suppresses_system_header_warnings_using_origin_snapshot() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("system.h", "VALUE");
        sources.set_system_header_from(file, 0, true).unwrap();
        let direct = Span::new(file, 0, 5);
        let origin = sources.intern_origin(
            OriginKind::MacroExpansion {
                macro_name: "VALUE".into(),
                invocation: direct,
                definition: direct,
            },
            OriginId::DIRECT,
            direct,
        );
        sources.set_system_header_from(file, 0, false).unwrap();

        let mut engine = DiagnosticEngine::new();
        assert_eq!(
            engine.emit(
                &sources,
                Diagnostic::warning("CCC0101", "pedantic", "extension")
                    .with_primary_span(direct.with_origin_id(origin))
            ),
            EmitOutcome::Suppressed
        );
        assert!(engine.diagnostics().is_empty());
    }

    #[test]
    fn halts_at_the_error_limit() {
        let sources = SourceMap::new();
        let mut engine = DiagnosticEngine::with_options(DiagnosticOptions {
            error_limit: 2,
            ..DiagnosticOptions::default()
        });

        assert_eq!(
            engine.emit(&sources, Diagnostic::error("CCC1", "first")),
            EmitOutcome::Emitted
        );
        assert_eq!(
            engine.emit(&sources, Diagnostic::error("CCC2", "second")),
            EmitOutcome::Emitted
        );
        assert_eq!(
            engine.emit(&sources, Diagnostic::error("CCC3", "third")),
            EmitOutcome::Halted
        );
        assert_eq!(engine.error_count(), 2);
        assert!(engine.is_halted());
        assert_eq!(engine.diagnostics().len(), 3);
        assert_eq!(engine.diagnostics()[2].code, "CCC0000");
    }

    #[test]
    fn renders_the_versioned_json_schema_deterministically() {
        let mut sources = SourceMap::new();
        let file = sources.add_file_occurrence(
            SourceFileSpec::new("spelled/input.c")
                .with_display_name("display/input.c")
                .with_resolved_path("/resolved/input.c"),
            "int value;\n",
        );
        let diagnostic = Diagnostic::error("CCC1020", "unexpected `value`")
            .with_category("syntax")
            .with_primary(Span::new(file, 4, 9), "while parsing")
            .with_secondary(Span::new(file, 0, 3), "declaration starts here")
            .with_note("recovered at `;`");

        assert_eq!(
            render_json_document(&[diagnostic], &sources, RenderOptions::default()),
            concat!(
                "{\"schema_version\":1,\"diagnostics\":[{",
                "\"severity\":\"error\",\"code\":\"CCC1020\",",
                "\"category\":\"syntax\",\"message\":\"unexpected `value`\",",
                "\"primary\":{\"label\":\"while parsing\",\"location\":{",
                "\"spelled_path\":\"spelled/input.c\",",
                "\"resolved_path\":\"/resolved/input.c\",",
                "\"display_path\":\"display/input.c\",",
                "\"start\":{\"byte\":4,\"line\":1,\"column\":5},",
                "\"end\":{\"byte\":9,\"line\":1,\"column\":10}}},",
                "\"secondary\":[{\"label\":\"declaration starts here\",\"location\":{",
                "\"spelled_path\":\"spelled/input.c\",",
                "\"resolved_path\":\"/resolved/input.c\",",
                "\"display_path\":\"display/input.c\",",
                "\"start\":{\"byte\":0,\"line\":1,\"column\":1},",
                "\"end\":{\"byte\":3,\"line\":1,\"column\":4}}}],",
                "\"notes\":[\"recovered at `;`\"],",
                "\"include_trace\":{\"truncated\":false,\"frames\":[]},",
                "\"macro_trace\":{\"truncated\":false,\"frames\":[]}}]}\n",
            )
        );
    }
}
