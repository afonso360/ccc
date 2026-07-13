use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::engine::{
    DependencyGraph, DiagnosticPragmaAction, MacroSnapshot, PpItem, PragmaEvent, PreprocessOutput,
};
use crate::lexer::tokens_require_separator;
use crate::token::PpToken;

/// Renders preprocessing output as valid C input.
pub fn render_preprocessed(output: &PreprocessOutput, suppress_line_markers: bool) -> String {
    let mut rendered = String::new();
    let mut previous_token = None::<&PpToken>;
    let mut at_start_of_line = true;
    for item in &output.items {
        match item {
            PpItem::Token(token) => {
                if !at_start_of_line
                    && (token.leading_space
                        || previous_token
                            .is_some_and(|previous| tokens_require_separator(previous, token)))
                {
                    rendered.push(' ');
                }
                rendered.push_str(&token.spelling);
                previous_token = Some(token);
                at_start_of_line = false;
            }
            PpItem::Newline => {
                prevent_line_splice(&mut rendered);
                rendered.push('\n');
                previous_token = None;
                at_start_of_line = true;
            }
            PpItem::LineMarker(marker) if !suppress_line_markers => {
                if !rendered.is_empty() && !rendered.ends_with('\n') {
                    prevent_line_splice(&mut rendered);
                    rendered.push('\n');
                }
                rendered.push_str("# ");
                rendered.push_str(&marker.line.to_string());
                rendered.push(' ');
                rendered.push_str(&quote_c_string(&marker.file));
                for flag in &marker.flags {
                    rendered.push(' ');
                    rendered.push_str(&flag.to_string());
                }
                rendered.push('\n');
                previous_token = None;
                at_start_of_line = true;
            }
            PpItem::LineMarker(_) => {}
            PpItem::Pragma(PragmaEvent::Once { .. } | PragmaEvent::SystemHeader { .. }) => {}
            PpItem::Pragma(pragma) => {
                if !rendered.is_empty() && !rendered.ends_with('\n') {
                    prevent_line_splice(&mut rendered);
                    rendered.push('\n');
                }
                rendered.push_str("#pragma ");
                rendered.push_str(&render_pragma(pragma));
                prevent_line_splice(&mut rendered);
                rendered.push('\n');
                previous_token = None;
                at_start_of_line = true;
            }
        }
    }
    rendered
}

fn prevent_line_splice(rendered: &mut String) {
    if rendered.ends_with('\\') {
        rendered.push(' ');
    }
}

pub fn render_macro_definitions(macros: &MacroSnapshot) -> String {
    let mut output = String::new();
    for definition in &macros.definitions {
        output.push_str("#define ");
        output.push_str(&definition.name);
        if let Some(parameters) = &definition.parameters {
            output.push('(');
            output.push_str(&parameters.join(", "));
            if definition.variadic {
                if !parameters.is_empty() {
                    output.push_str(", ");
                }
                output.push_str("...");
            }
            output.push(')');
        }
        if !definition.replacement.is_empty() {
            output.push(' ');
            output.push_str(&render_token_sequence(&definition.replacement));
        }
        prevent_line_splice(&mut output);
        output.push('\n');
    }
    output
}

fn render_pragma(pragma: &PragmaEvent) -> String {
    match pragma {
        PragmaEvent::Once { .. } => "once".to_owned(),
        PragmaEvent::SystemHeader { .. } => "GCC system_header".to_owned(),
        PragmaEvent::Diagnostic { action, option, .. } => {
            let action = match action {
                DiagnosticPragmaAction::Push => "push",
                DiagnosticPragmaAction::Pop => "pop",
                DiagnosticPragmaAction::Ignored => "ignored",
                DiagnosticPragmaAction::Warning => "warning",
                DiagnosticPragmaAction::Error => "error",
            };
            option.as_ref().map_or_else(
                || format!("GCC diagnostic {action}"),
                |option| format!("GCC diagnostic {action} {}", quote_c_string(option)),
            )
        }
        PragmaEvent::Pack { payload, .. } => {
            if payload.is_empty() {
                "pack".to_owned()
            } else {
                format!("pack {}", render_token_sequence(payload))
            }
        }
        PragmaEvent::Unknown { text, .. } => text.clone(),
    }
}

fn render_token_sequence(tokens: &[PpToken]) -> String {
    let mut output = String::new();
    let mut previous = None;
    for token in tokens {
        if !output.is_empty()
            && (token.leading_space
                || previous.is_some_and(|previous| tokens_require_separator(previous, token)))
        {
            output.push(' ');
        }
        output.push_str(&token.spelling);
        previous = Some(token);
    }
    output
}

fn quote_c_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyRenderOptions {
    pub targets: Vec<String>,
    pub quote_targets: bool,
    pub phony_targets: bool,
    pub exclude_system_headers: bool,
    pub wrap_column: usize,
}

impl Default for DependencyRenderOptions {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            quote_targets: true,
            phony_targets: false,
            exclude_system_headers: false,
            wrap_column: 80,
        }
    }
}

pub fn render_dependencies(graph: &DependencyGraph, options: &DependencyRenderOptions) -> String {
    let Some(main) = graph.files.first() else {
        return String::new();
    };
    let targets = if options.targets.is_empty() {
        vec![default_object_target(&main.path)]
    } else {
        options.targets.clone()
    };
    let targets = targets
        .iter()
        .map(|target| {
            if options.quote_targets {
                escape_make(target)
            } else {
                target.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut seen = BTreeSet::new();
    let dependencies = graph
        .files
        .iter()
        .enumerate()
        .filter(|(index, dependency)| {
            *index == 0 || !options.exclude_system_headers || !dependency.system
        })
        .map(|(_, dependency)| dependency)
        .filter(|dependency| seen.insert(dependency.path.clone()))
        .map(|dependency| escape_make(&dependency.path.to_string_lossy()))
        .collect::<Vec<_>>();

    let mut output = format!("{targets}:");
    let mut column = output.len();
    for dependency in &dependencies {
        let required = dependency.len() + 1;
        if options.wrap_column > 0
            && column + required > options.wrap_column
            && column > targets.len() + 1
        {
            output.push_str(" \\\n  ");
            column = 2;
        } else {
            output.push(' ');
            column += 1;
        }
        output.push_str(dependency);
        column += dependency.len();
    }
    output.push('\n');
    if options.phony_targets {
        for dependency in dependencies.iter().skip(1) {
            output.push('\n');
            output.push_str(dependency);
            output.push_str(":\n");
        }
    }
    output
}

fn default_object_target(input: &Path) -> String {
    let mut target = PathBuf::from(input.file_name().unwrap_or(input.as_os_str()));
    target.set_extension("o");
    target.to_string_lossy().into_owned()
}

fn escape_make(text: &str) -> String {
    let mut output = String::new();
    let mut preceding_backslashes = 0_usize;
    for character in text.chars() {
        match character {
            '\\' => {
                output.push('\\');
                preceding_backslashes += 1;
            }
            ' ' | '\t' | '#' => {
                output.extend(std::iter::repeat_n('\\', preceding_backslashes + 1));
                output.push(character);
                preceding_backslashes = 0;
            }
            '$' => {
                output.push_str("$$");
                preceding_backslashes = 0;
            }
            character => {
                output.push(character);
                preceding_backslashes = 0;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Dependency, DependencyGraph, MacroSnapshotEntry};
    use crate::token::PpTokenKind;

    #[test]
    fn consumes_preprocessor_only_pragmas_but_preserves_downstream_pragmas() {
        let mut sources = ccc_session::SourceMap::new();
        let file = sources.add_file("pragma.c", "");
        let span = ccc_session::Span::new(file, 0, 0);
        let output = PreprocessOutput {
            items: vec![
                PpItem::Pragma(PragmaEvent::Once { span }),
                PpItem::Pragma(PragmaEvent::SystemHeader { span }),
                PpItem::Pragma(PragmaEvent::Diagnostic {
                    action: DiagnosticPragmaAction::Push,
                    option: None,
                    span,
                }),
                PpItem::Pragma(PragmaEvent::Unknown {
                    text: "vendor payload".to_owned(),
                    span,
                }),
            ],
            ..PreprocessOutput::default()
        };

        assert_eq!(
            render_preprocessed(&output, true),
            "#pragma GCC diagnostic push\n#pragma vendor payload\n"
        );
    }

    #[test]
    fn keeps_a_catch_all_backslash_from_splicing_the_rendered_newline() {
        let mut sources = ccc_session::SourceMap::new();
        let file = sources.add_file("backslash.c", "");
        let span = ccc_session::Span::new(file, 0, 0);
        let output = PreprocessOutput {
            items: vec![
                PpItem::Token(PpToken::synthetic(PpTokenKind::Punctuator, span, "\\")),
                PpItem::Newline,
                PpItem::Token(PpToken::synthetic(PpTokenKind::Identifier, span, "after")),
                PpItem::Newline,
            ],
            ..PreprocessOutput::default()
        };

        assert_eq!(render_preprocessed(&output, true), "\\ \nafter\n");
    }

    #[test]
    fn keeps_pragma_and_macro_dump_backslashes_from_splicing_lines() {
        let mut sources = ccc_session::SourceMap::new();
        let file = sources.add_file("backslash.c", "");
        let span = ccc_session::Span::new(file, 0, 0);
        let output = PreprocessOutput {
            items: vec![
                PpItem::Pragma(PragmaEvent::Unknown {
                    text: "vendor \\".to_owned(),
                    span,
                }),
                PpItem::Token(PpToken::synthetic(PpTokenKind::Identifier, span, "after")),
                PpItem::Newline,
            ],
            ..PreprocessOutput::default()
        };
        assert_eq!(
            render_preprocessed(&output, true),
            "#pragma vendor \\ \nafter\n"
        );

        let macros = MacroSnapshot {
            definitions: vec![
                MacroSnapshotEntry {
                    name: "TRAILING".to_owned(),
                    parameters: None,
                    variadic: false,
                    replacement: vec![PpToken::synthetic(PpTokenKind::Punctuator, span, "\\")],
                },
                MacroSnapshotEntry {
                    name: "NEXT".to_owned(),
                    parameters: None,
                    variadic: false,
                    replacement: vec![PpToken::synthetic(PpTokenKind::PpNumber, span, "1")],
                },
            ],
        };
        assert_eq!(
            render_macro_definitions(&macros),
            "#define TRAILING \\ \n#define NEXT 1\n"
        );
    }

    #[test]
    fn escapes_make_dependencies_and_emits_phony_targets() {
        let graph = DependencyGraph {
            files: vec![
                Dependency {
                    path: PathBuf::from("src/a b.c"),
                    system: false,
                },
                Dependency {
                    path: PathBuf::from("inc/x#y.h"),
                    system: false,
                },
            ],
            edges: Vec::new(),
        };
        let rendered = render_dependencies(
            &graph,
            &DependencyRenderOptions {
                targets: vec!["out file.o".to_owned()],
                phony_targets: true,
                wrap_column: 0,
                ..DependencyRenderOptions::default()
            },
        );
        assert_eq!(
            rendered,
            "out\\ file.o: src/a\\ b.c inc/x\\#y.h\n\ninc/x\\#y.h:\n"
        );
    }

    #[test]
    fn wraps_dependencies_with_make_continuations() {
        let graph = DependencyGraph {
            files: vec![
                Dependency {
                    path: PathBuf::from("source.c"),
                    system: false,
                },
                Dependency {
                    path: PathBuf::from("long-header-name.h"),
                    system: false,
                },
            ],
            edges: Vec::new(),
        };
        let rendered = render_dependencies(
            &graph,
            &DependencyRenderOptions {
                wrap_column: 20,
                ..DependencyRenderOptions::default()
            },
        );
        assert_eq!(rendered, "source.o: source.c \\\n  long-header-name.h\n");
    }
}
