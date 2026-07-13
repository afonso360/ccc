//! Structured diagnostics and deterministic text rendering.

use std::fmt;

use ccc_session::{SourceLocation, SourceMap, Span};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub primary: Option<PrimarySpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimarySpan {
    pub span: Span,
    pub label: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: code.into(),
            message: message.into(),
            primary: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    pub fn with_primary(mut self, span: Span, label: impl Into<String>) -> Self {
        self.primary = Some(PrimarySpan {
            span,
            label: Some(label.into()),
        });
        self
    }

    /// Renders the primary span with a source line and caret.
    pub fn render(&self, sources: &SourceMap) -> String {
        let mut rendered = format!(
            "{}[{}]: {}\n",
            self.severity.as_str(),
            self.code,
            self.message
        );

        let Some(primary) = &self.primary else {
            return rendered;
        };
        let Some(name) = sources.file_name(primary.span.file) else {
            return rendered;
        };
        let Some(start) = sources.location(primary.span.file, primary.span.start) else {
            return rendered;
        };
        let Some(source_line) = sources.line_text(primary.span.file, start.line) else {
            return rendered;
        };

        let gutter_width = start.line.to_string().len();
        rendered.push_str(&format!(" --> {name}:{}:{}\n", start.line, start.column));
        rendered.push_str(&format!(" {:>gutter_width$} |\n", ""));
        rendered.push_str(&format!(" {:>gutter_width$} | {source_line}\n", start.line));
        rendered.push_str(&format!(" {:>gutter_width$} | ", ""));
        rendered.push_str(&caret_padding(source_line, start.column));
        rendered.push_str(&"^".repeat(caret_width(sources, primary.span, start)));
        if let Some(label) = &primary.label {
            rendered.push(' ');
            rendered.push_str(label);
        }
        rendered.push('\n');
        rendered
    }
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
        write!(
            formatter,
            "{}[{}]: {}",
            self.severity.as_str(),
            self.code,
            self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

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
}
