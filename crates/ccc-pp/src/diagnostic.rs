use ccc_session::Span;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PpSeverity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PpDiagnosticCategory {
    General,
    Trigraph,
    MacroRedefined,
    UnknownPragma,
    SystemHeader,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpSecondarySpan {
    pub span: Span,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpDiagnostic {
    pub severity: PpSeverity,
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
    pub category: PpDiagnosticCategory,
    pub is_system_header: bool,
    pub secondary: Vec<PpSecondarySpan>,
    pub notes: Vec<String>,
}

impl PpDiagnostic {
    pub fn new(severity: PpSeverity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            span: None,
            category: PpDiagnosticCategory::General,
            is_system_header: false,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(PpSeverity::Error, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(PpSeverity::Warning, code, message)
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_category(mut self, category: PpDiagnosticCategory) -> Self {
        self.category = category;
        self
    }

    pub fn in_system_header(mut self, is_system_header: bool) -> Self {
        self.is_system_header = is_system_header;
        self
    }

    pub fn with_secondary(mut self, span: Span, label: impl Into<String>) -> Self {
        self.secondary.push(PpSecondarySpan {
            span,
            label: Some(label.into()),
        });
        self
    }

    pub fn with_secondary_span(mut self, span: Span) -> Self {
        self.secondary.push(PpSecondarySpan { span, label: None });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// A driver-owned destination for preprocessing diagnostics.
pub trait DiagnosticSink {
    fn emit(&mut self, diagnostic: PpDiagnostic);

    fn handle_pragma(&mut self, _pragma: &crate::engine::PragmaEvent) {}

    fn has_errors(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VecDiagnosticSink {
    pub diagnostics: Vec<PpDiagnostic>,
}

impl VecDiagnosticSink {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == PpSeverity::Error)
    }
}

impl DiagnosticSink for VecDiagnosticSink {
    fn emit(&mut self, diagnostic: PpDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn has_errors(&self) -> bool {
        VecDiagnosticSink::has_errors(self)
    }
}
