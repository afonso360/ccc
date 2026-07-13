//! Structured diagnostics, warning policy, and deterministic text rendering.

use std::collections::HashMap;
use std::fmt;

use ccc_session::{OriginKind, SourceLocation, SourceMap, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
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
        Self::new(Severity::Warning, code, message).with_category(category)
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
    let category = diagnostic
        .category
        .as_ref()
        .map(|category| format!(" [-W{category}]"))
        .unwrap_or_default();
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
            self.diagnostics.push(Diagnostic::error(
                "CCC0000",
                format!(
                    "too many errors emitted; stopping after {}",
                    self.options.error_limit
                ),
            ));
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
}
