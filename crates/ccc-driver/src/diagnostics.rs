use ccc_diag::{
    Diagnostic, DiagnosticEngine, DiagnosticOptions, Severity, WarningCategory, WarningLevel,
};
use ccc_pp::{
    DiagnosticPragmaAction, DiagnosticSink, PpDiagnostic, PpDiagnosticCategory, PpSeverity,
    PragmaEvent,
};
use ccc_session::SourceMap;

use crate::warnings::WarningPolicy;

/// Captures the warning state that is active at each preprocessing diagnostic.
/// Preprocessor-specific system-header exemptions are applied before common
/// warning promotion and error-limit policy enter the shared engine.
#[derive(Debug)]
pub(crate) struct PreprocessorDiagnostics {
    engine: DiagnosticEngine,
    warn_in_system_headers: bool,
}

impl PreprocessorDiagnostics {
    pub(crate) fn new(
        warning_policy: &WarningPolicy,
        warnings_enabled: bool,
        warnings_as_errors: bool,
        warn_in_system_headers: bool,
        error_limit: usize,
    ) -> Self {
        let mut diagnostics = Self {
            engine: DiagnosticEngine::with_options(DiagnosticOptions {
                warnings_enabled,
                warnings_as_errors,
                // Preprocessing owns this filter because directive-generated
                // warnings have exemptions the shared engine cannot infer.
                suppress_warnings_in_system_headers: false,
                error_limit,
                ..DiagnosticOptions::default()
            }),
            warn_in_system_headers,
        };
        for category in ["cpp", "trigraphs", "macro-redefined", "unknown-pragmas"] {
            diagnostics.set_level(category, warning_policy.level(category));
        }
        diagnostics
    }

    fn set_level(&mut self, category: &str, level: WarningLevel) {
        self.engine
            .set_warning_level(normalize_warning_name(category), level);
    }

    pub(crate) fn finish(self) -> DiagnosticEngine {
        self.engine
    }
}

impl DiagnosticSink for PreprocessorDiagnostics {
    fn emit(&mut self, diagnostic: PpDiagnostic) {
        if diagnostic.severity == PpSeverity::Warning
            && diagnostic.is_system_header
            && !self.warn_in_system_headers
            && diagnostic.code != "CCC1315"
        {
            return;
        }
        let category = category_name(diagnostic.category);
        let diagnostic = convert(diagnostic, category);
        let _ = self.engine.emit(&SourceMap::new(), diagnostic);
    }

    fn handle_pragma(&mut self, pragma: &PragmaEvent) {
        let PragmaEvent::Diagnostic { action, option, .. } = pragma else {
            return;
        };
        match action {
            DiagnosticPragmaAction::Push => {
                self.engine.push_warning_state();
            }
            DiagnosticPragmaAction::Pop => {
                let _ = self.engine.pop_warning_state();
            }
            DiagnosticPragmaAction::Ignored
            | DiagnosticPragmaAction::Warning
            | DiagnosticPragmaAction::Error => {
                let Some(option) = option else {
                    return;
                };
                let level = match action {
                    DiagnosticPragmaAction::Ignored => WarningLevel::Ignored,
                    DiagnosticPragmaAction::Warning => WarningLevel::Warning,
                    DiagnosticPragmaAction::Error => WarningLevel::Error,
                    DiagnosticPragmaAction::Push | DiagnosticPragmaAction::Pop => unreachable!(),
                };
                self.set_level(option, level);
            }
        }
    }

    fn has_errors(&self) -> bool {
        self.engine.has_errors()
    }

    fn is_halted(&self) -> bool {
        self.engine.is_halted()
    }
}

fn convert(diagnostic: PpDiagnostic, category: &'static str) -> Diagnostic {
    let severity = match diagnostic.severity {
        PpSeverity::Error => Severity::Error,
        PpSeverity::Warning => Severity::Warning,
        PpSeverity::Note => Severity::Note,
    };
    let mut converted = if severity == Severity::Warning {
        Diagnostic::warning(
            diagnostic.code,
            WarningCategory::new(category),
            diagnostic.message,
        )
    } else {
        Diagnostic::new(severity, diagnostic.code, diagnostic.message)
            .with_category(WarningCategory::new(category))
    };
    if let Some(span) = diagnostic.span {
        converted = converted.with_primary_span(span);
    }
    for secondary in diagnostic.secondary {
        converted = if let Some(label) = secondary.label {
            converted.with_secondary(secondary.span, label)
        } else {
            converted.with_secondary_span(secondary.span)
        };
    }
    for note in diagnostic.notes {
        converted = converted.with_note(note);
    }
    converted
}

fn category_name(category: PpDiagnosticCategory) -> &'static str {
    match category {
        PpDiagnosticCategory::General => "cpp",
        PpDiagnosticCategory::Trigraph => "trigraphs",
        PpDiagnosticCategory::MacroRedefined => "macro-redefined",
        PpDiagnosticCategory::UnknownPragma => "unknown-pragmas",
        PpDiagnosticCategory::SystemHeader => "system-headers",
    }
}

fn normalize_warning_name(category: &str) -> String {
    category.strip_prefix("-W").unwrap_or(category).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccc_pp::PpDiagnostic;

    #[test]
    fn captures_source_ordered_diagnostic_pragma_state() {
        let policy = WarningPolicy::new(false, false, &[]);
        let mut sink = PreprocessorDiagnostics::new(&policy, true, false, false, 20);
        sink.handle_pragma(&PragmaEvent::Diagnostic {
            action: DiagnosticPragmaAction::Push,
            option: None,
            span: test_span(),
        });
        sink.handle_pragma(&PragmaEvent::Diagnostic {
            action: DiagnosticPragmaAction::Ignored,
            option: Some("-Wunknown-pragmas".to_owned()),
            span: test_span(),
        });
        sink.emit(
            PpDiagnostic::warning("CCC1", "hidden")
                .with_category(PpDiagnosticCategory::UnknownPragma),
        );
        sink.handle_pragma(&PragmaEvent::Diagnostic {
            action: DiagnosticPragmaAction::Pop,
            option: None,
            span: test_span(),
        });
        sink.emit(
            PpDiagnostic::warning("CCC2", "visible")
                .with_category(PpDiagnosticCategory::UnknownPragma),
        );

        let engine = sink.finish();
        assert_eq!(engine.diagnostics().len(), 1);
        assert_eq!(engine.diagnostics()[0].message, "visible");
    }

    fn test_span() -> ccc_session::Span {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "");
        ccc_session::Span::new(file, 0, 0)
    }
}
