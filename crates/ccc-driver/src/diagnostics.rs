use std::collections::HashMap;

use ccc_diag::{
    Diagnostic, DiagnosticEngine, DiagnosticOptions, Severity, WarningCategory, WarningLevel,
};
use ccc_pp::{
    DiagnosticPragmaAction, DiagnosticSink, PpDiagnostic, PpDiagnosticCategory, PpSeverity,
    PragmaEvent,
};
use ccc_session::SourceMap;

use crate::warnings::WarningPolicy;

#[derive(Clone, Debug)]
struct PendingDiagnostic {
    diagnostic: PpDiagnostic,
    warning_level: WarningLevel,
}

/// Captures the warning state that is active at each preprocessing diagnostic.
/// Once preprocessing releases the source map, this sink applies
/// preprocessor-specific system-header exemptions and delegates common warning
/// promotion and error-limit policy to the shared diagnostic engine.
#[derive(Debug)]
pub(crate) struct PreprocessorDiagnostics {
    diagnostics: Vec<PendingDiagnostic>,
    warning_levels: HashMap<String, WarningLevel>,
    warning_stack: Vec<HashMap<String, WarningLevel>>,
    raw_errors: usize,
}

impl PreprocessorDiagnostics {
    pub(crate) fn new(warning_policy: &WarningPolicy) -> Self {
        let mut diagnostics = Self {
            diagnostics: Vec::new(),
            warning_levels: HashMap::new(),
            warning_stack: Vec::new(),
            raw_errors: 0,
        };
        for category in ["cpp", "trigraphs", "macro-redefined", "unknown-pragmas"] {
            diagnostics.set_level(category, warning_policy.level(category));
        }
        diagnostics
    }

    fn set_level(&mut self, category: &str, level: WarningLevel) {
        self.warning_levels
            .insert(normalize_warning_name(category), level);
    }

    fn level(&self, category: PpDiagnosticCategory) -> WarningLevel {
        self.warning_levels
            .get(category_name(category))
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn finish(
        self,
        sources: &SourceMap,
        warnings_enabled: bool,
        warnings_as_errors: bool,
        warn_in_system_headers: bool,
        error_limit: usize,
    ) -> DiagnosticEngine {
        let mut engine = DiagnosticEngine::with_options(DiagnosticOptions {
            warnings_enabled,
            warnings_as_errors,
            // Preprocessing owns this filter because directive-generated
            // warnings have GCC-compatible exemptions that the shared engine
            // cannot infer after conversion.
            suppress_warnings_in_system_headers: false,
            error_limit,
            ..DiagnosticOptions::default()
        });

        for pending in self.diagnostics {
            let category = category_name(pending.diagnostic.category);
            if pending.diagnostic.severity == PpSeverity::Warning
                && pending.diagnostic.is_system_header
                && !warn_in_system_headers
                && pending.diagnostic.code != "CCC1315"
            {
                continue;
            }
            if pending.diagnostic.severity == PpSeverity::Warning {
                engine.set_warning_level(category, pending.warning_level);
            }
            let diagnostic = convert(pending.diagnostic, category);
            let _ = engine.emit(sources, diagnostic);
            if engine.is_halted() {
                break;
            }
        }
        engine
    }
}

impl DiagnosticSink for PreprocessorDiagnostics {
    fn emit(&mut self, diagnostic: PpDiagnostic) {
        if diagnostic.severity == PpSeverity::Error {
            self.raw_errors += 1;
        }
        let warning_level = self.level(diagnostic.category);
        if diagnostic.severity != PpSeverity::Warning || warning_level != WarningLevel::Ignored {
            self.diagnostics.push(PendingDiagnostic {
                diagnostic,
                warning_level,
            });
        }
    }

    fn handle_pragma(&mut self, pragma: &PragmaEvent) {
        let PragmaEvent::Diagnostic { action, option, .. } = pragma else {
            return;
        };
        match action {
            DiagnosticPragmaAction::Push => {
                self.warning_stack.push(self.warning_levels.clone());
            }
            DiagnosticPragmaAction::Pop => {
                if let Some(levels) = self.warning_stack.pop() {
                    self.warning_levels = levels;
                }
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
        self.raw_errors != 0
    }
}

fn convert(diagnostic: PpDiagnostic, category: &'static str) -> Diagnostic {
    let severity = match diagnostic.severity {
        PpSeverity::Error => Severity::Error,
        PpSeverity::Warning => Severity::Warning,
        PpSeverity::Note => Severity::Note,
    };
    let mut converted = Diagnostic::new(severity, diagnostic.code, diagnostic.message);
    if severity == Severity::Warning {
        converted = converted.with_category(WarningCategory::new(category));
    }
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
        let mut sink = PreprocessorDiagnostics::new(&policy);
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

        let engine = sink.finish(&SourceMap::new(), true, false, false, 20);
        assert_eq!(engine.diagnostics().len(), 1);
        assert_eq!(engine.diagnostics()[0].message, "visible");
    }

    fn test_span() -> ccc_session::Span {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "");
        ccc_session::Span::new(file, 0, 0)
    }
}
