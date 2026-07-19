use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use ccc_session::{FileId, Session, SourceFileSpec, Span};
use ccc_target::{CapabilityKind, LanguageMode, SystemIncludeKind};

use crate::condition::{evaluate as evaluate_condition, parse_header_operand};
use crate::diagnostic::{DiagnosticSink, PpDiagnostic, PpDiagnosticCategory, PpSeverity};
use crate::files::{FileIdentity, FileProvider, LoadedFile};
use crate::lexer::{LexerOptions, lex_file, lex_fragment};
use crate::macros::{
    DefineResult, ExpansionLocation, MacroDefinition, MacroForm, MacroTable, canonical_macro_name,
    expand, parse_pragma_operators, redefinition_diagnostic,
};
use crate::options::{CommandLineMacro, IncludePathKind, PreprocessOptions};
use crate::token::{PpToken, PpTokenKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PpItem {
    Token(PpToken),
    Pragma(PragmaEvent),
    LineMarker(LineMarker),
    Newline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineMarker {
    pub line: usize,
    pub file: String,
    pub flags: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PragmaEvent {
    Once {
        span: Span,
    },
    SystemHeader {
        span: Span,
    },
    Diagnostic {
        action: DiagnosticPragmaAction,
        option: Option<String>,
        span: Span,
    },
    GccOptimize {
        payload: Vec<PpToken>,
        span: Span,
    },
    Pack {
        payload: Vec<PpToken>,
        span: Span,
    },
    Unknown {
        text: String,
        span: Span,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticPragmaAction {
    Push,
    Pop,
    Ignored,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dependency {
    pub path: PathBuf,
    pub system: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyEdge {
    pub from: PathBuf,
    pub to: PathBuf,
    pub spelled: String,
    pub system: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DependencyGraph {
    pub files: Vec<Dependency>,
    pub edges: Vec<DependencyEdge>,
}

impl DependencyGraph {
    fn record_file(&mut self, path: PathBuf, system: bool) -> usize {
        let index = self.files.len();
        self.files.push(Dependency { path, system });
        index
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacroSnapshotEntry {
    pub name: String,
    pub parameters: Option<Vec<String>>,
    pub variadic: bool,
    pub replacement: Vec<PpToken>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MacroSnapshot {
    pub definitions: Vec<MacroSnapshotEntry>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreprocessOutput {
    /// Canonical ordered preprocessing output, including pragma and file events.
    pub items: Vec<PpItem>,
    pub macros: MacroSnapshot,
    pub dependencies: DependencyGraph,
    pub had_errors: bool,
}

impl PreprocessOutput {
    pub fn tokens(&self) -> Vec<PpToken> {
        self.items
            .iter()
            .filter_map(|item| match item {
                PpItem::Token(token) => Some(token.clone()),
                _ => None,
            })
            .collect()
    }
}

pub struct PreprocessContext<'a> {
    pub session: &'a mut Session,
    pub diagnostics: &'a mut dyn DiagnosticSink,
    pub options: &'a PreprocessOptions,
    pub files: &'a dyn FileProvider,
}

pub fn preprocess(context: &mut PreprocessContext<'_>, main_file: FileId) -> PreprocessOutput {
    let mut options = context.options.clone();
    options.language_mode = context.session.config.language.mode;
    options.gnu_comma_elision &= options.language_mode == LanguageMode::Gnu11;
    if options.trigraphs.is_none() {
        options.trigraphs = Some(context.session.config.language.trigraphs_enabled());
    }
    for kind in [
        CapabilityKind::Attribute,
        CapabilityKind::Builtin,
        CapabilityKind::Extension,
        CapabilityKind::Feature,
    ] {
        for (key, entry) in context.session.config.capabilities.iter() {
            if key.kind == kind {
                options
                    .features
                    .entry(key.name.clone())
                    .or_insert_with(|| entry.state.is_available());
            }
        }
    }

    let search_entries = build_search_entries(context.session, &options);
    let mut engine = Engine {
        session: context.session,
        diagnostics: context.diagnostics,
        options,
        files: context.files,
        search_entries,
        macros: MacroTable::default(),
        items: Vec::new(),
        dependencies: DependencyGraph::default(),
        once_files: BTreeSet::new(),
        include_stack: Vec::new(),
        error_count: 0,
        diagnostic_count: 0,
        diagnostic_limit_reported: false,
    };
    engine.run(main_file)
}

#[derive(Clone, Debug)]
struct SearchEntry {
    path: PathBuf,
    system: bool,
    quote_only: bool,
}

fn build_search_entries(session: &Session, options: &PreprocessOptions) -> Vec<SearchEntry> {
    let mut entries = Vec::new();
    push_option_entries(&mut entries, options, IncludePathKind::Quote, false);
    for entry in session
        .config
        .system_includes()
        .iter()
        .filter(|entry| entry.kind == SystemIncludeKind::Quote)
    {
        entries.push(SearchEntry {
            path: entry.path.clone(),
            system: false,
            quote_only: true,
        });
    }
    push_option_entries(&mut entries, options, IncludePathKind::User, false);
    push_option_entries(&mut entries, options, IncludePathKind::System, true);
    push_option_entries(&mut entries, options, IncludePathKind::Resource, true);
    if let Some(resource_dir) = &session.config.resource_dir {
        let path = resource_dir.join("include");
        if !entries.iter().any(|entry| entry.path == path) {
            entries.push(SearchEntry {
                path,
                system: true,
                quote_only: false,
            });
        }
    }
    for kind in [
        SystemIncludeKind::Builtin,
        SystemIncludeKind::System,
        SystemIncludeKind::Framework,
    ] {
        for entry in session
            .config
            .system_includes()
            .iter()
            .filter(|entry| entry.kind == kind)
        {
            entries.push(SearchEntry {
                path: entry.path.clone(),
                system: true,
                quote_only: false,
            });
        }
    }
    push_option_entries(&mut entries, options, IncludePathKind::After, true);
    for entry in session
        .config
        .system_includes()
        .iter()
        .filter(|entry| entry.kind == SystemIncludeKind::After)
    {
        entries.push(SearchEntry {
            path: entry.path.clone(),
            system: true,
            quote_only: false,
        });
    }
    let mut seen = BTreeSet::new();
    entries.retain(|entry| seen.insert((entry.path.clone(), entry.system, entry.quote_only)));
    entries
}

fn push_option_entries(
    entries: &mut Vec<SearchEntry>,
    options: &PreprocessOptions,
    kind: IncludePathKind,
    system: bool,
) {
    entries.extend(
        options
            .include_paths
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| SearchEntry {
                path: entry.path.clone(),
                system,
                quote_only: kind == IncludePathKind::Quote,
            }),
    );
}

#[derive(Clone, Debug)]
struct FileFrame {
    file: FileId,
    path: PathBuf,
    identity: FileIdentity,
    found_entry: Option<usize>,
    system: bool,
    dependency_index: usize,
    dependency_edge: Option<usize>,
    is_main: bool,
}

fn mapped_source_identity(spec: Option<&SourceFileSpec>) -> Option<FileIdentity> {
    match spec?.identity.as_ref()? {
        ccc_session::FileIdentity::CanonicalPath(path) => {
            Some(FileIdentity(path.to_string_lossy().into_owned()))
        }
        ccc_session::FileIdentity::DeviceInode { device, inode } => {
            Some(FileIdentity(format!("{device}:{inode}")))
        }
        ccc_session::FileIdentity::Opaque(identity) => Some(FileIdentity(identity.clone())),
    }
}

struct Engine<'a> {
    session: &'a mut Session,
    diagnostics: &'a mut dyn DiagnosticSink,
    options: PreprocessOptions,
    files: &'a dyn FileProvider,
    search_entries: Vec<SearchEntry>,
    macros: MacroTable,
    items: Vec<PpItem>,
    dependencies: DependencyGraph,
    once_files: BTreeSet<FileIdentity>,
    include_stack: Vec<FileFrame>,
    error_count: usize,
    diagnostic_count: usize,
    diagnostic_limit_reported: bool,
}

impl Engine<'_> {
    fn run(&mut self, main_file: FileId) -> PreprocessOutput {
        let reference = Span::new(main_file, 0, 0);
        if !self.options.preprocessed_input {
            self.install_predefined_macros(reference);
            self.apply_command_line_macros(reference);
        }

        let main_name = self
            .session
            .sources
            .file_name(main_file)
            .unwrap_or("<input>")
            .to_owned();
        let main_path = self.session.sources.file_spec(main_file).map_or_else(
            || PathBuf::from(&main_name),
            |spec| spec.resolved_path.clone(),
        );
        let main_identity = mapped_source_identity(self.session.sources.file_spec(main_file))
            .or_else(|| {
                self.files
                    .read(&main_path)
                    .ok()
                    .map(|loaded| loaded.identity)
            })
            .unwrap_or_else(|| FileIdentity(format!("main:{}", main_path.display())));
        let main_dependency = self.dependencies.record_file(main_path.clone(), false);

        if !self.options.preprocessed_input {
            for path in self.options.imacros.clone() {
                self.process_forced_path(&path, main_file, false);
            }
            for path in self.options.forced_includes.clone() {
                self.process_forced_path(&path, main_file, true);
            }
        }
        self.process_file(
            FileFrame {
                file: main_file,
                path: main_path,
                identity: main_identity,
                found_entry: None,
                system: false,
                dependency_index: main_dependency,
                dependency_edge: None,
                is_main: true,
            },
            true,
        );

        let macros = MacroSnapshot {
            definitions: self
                .macros
                .definitions()
                .map(|(name, definition)| MacroSnapshotEntry {
                    name: name.to_owned(),
                    parameters: match &definition.form {
                        MacroForm::Object => None,
                        MacroForm::Function { parameters, .. } => Some(parameters.clone()),
                    },
                    variadic: matches!(definition.form, MacroForm::Function { variadic: true, .. }),
                    replacement: definition.replacement.clone(),
                })
                .collect(),
        };
        PreprocessOutput {
            items: std::mem::take(&mut self.items),
            macros,
            dependencies: std::mem::take(&mut self.dependencies),
            had_errors: self.error_count != 0 || self.diagnostics.has_errors(),
        }
    }

    fn install_predefined_macros(&mut self, reference: Span) {
        let mut definitions = self
            .session
            .config
            .frontend_predefined_macros()
            .into_iter()
            .collect::<Vec<_>>();
        definitions.extend(
            self.options
                .predefined_macros
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        for (name, replacement) in definitions {
            self.define_text(&name, &replacement, reference, true);
        }
    }

    fn apply_command_line_macros(&mut self, reference: Span) {
        for action in self.options.command_line_macros.clone() {
            match action {
                CommandLineMacro::Define(specification) => {
                    let definition = if specification.contains('=') {
                        specification.replacen('=', " ", 1)
                    } else {
                        format!("{specification} 1")
                    };
                    match lex_fragment(reference, &definition) {
                        Ok(tokens) => self.handle_define(&tokens),
                        Err(error) => self.emit(
                            PpDiagnostic::error(error.code, error.message).with_span(reference),
                        ),
                    }
                }
                CommandLineMacro::Undefine(name) => {
                    if let Some(name) = canonical_macro_name(&name) {
                        self.macros.remove(&name);
                    } else {
                        self.emit(PpDiagnostic::error(
                            "CCC1301",
                            format!("invalid command-line macro name '{name}'"),
                        ));
                    }
                }
            }
        }
    }

    fn define_text(&mut self, name: &str, replacement: &str, span: Span, predefined: bool) {
        let replacement = match lex_fragment(span, replacement) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.emit(PpDiagnostic::error(error.code, error.message).with_span(span));
                return;
            }
        };
        let result = self.macros.define(MacroDefinition {
            name: name.to_owned(),
            form: MacroForm::Object,
            replacement,
            definition_span: span,
            predefined,
        });
        if let DefineResult::Replaced(previous) = result {
            self.emit(redefinition_diagnostic(
                self.macros.get(name).expect("definition was installed"),
                &previous,
            ));
        }
    }

    fn process_forced_path(&mut self, path: &Path, parent: FileId, emit_tokens: bool) {
        let parent_path = self.session.sources.file_spec(parent).map_or_else(
            || PathBuf::from("<input>"),
            |spec| spec.resolved_path.clone(),
        );
        let directly_loaded = match read_header_candidate(self.files, path) {
            Ok(loaded) => loaded,
            Err(failure) => {
                self.emit(PpDiagnostic::error(
                    "CCC1302",
                    format!(
                        "cannot read forced input '{}' at '{}': {}",
                        path.display(),
                        failure.path.display(),
                        failure.error
                    ),
                ));
                return;
            }
        };
        let resolved = if let Some(loaded) = directly_loaded {
            Some(ResolvedHeader {
                loaded,
                entry_index: None,
                system: false,
            })
        } else if let Some(header) = path.to_str() {
            match resolve_header(self.files, &self.search_entries, None, header, false, false) {
                Ok(resolved) => resolved,
                Err(failure) => {
                    self.emit(PpDiagnostic::error(
                        "CCC1302",
                        format!(
                            "cannot read forced input '{}' at '{}': {}",
                            path.display(),
                            failure.path.display(),
                            failure.error
                        ),
                    ));
                    return;
                }
            }
        } else {
            None
        };
        match resolved {
            Some(resolved) => {
                let dependency_index = self
                    .dependencies
                    .record_file(resolved.loaded.path.clone(), resolved.system);
                let dependency_edge = self.dependencies.edges.len();
                self.dependencies.edges.push(DependencyEdge {
                    from: parent_path,
                    to: resolved.loaded.path.clone(),
                    spelled: path.to_string_lossy().into_owned(),
                    system: resolved.system,
                });
                let mut spec = SourceFileSpec::new(path)
                    .with_resolved_path(&resolved.loaded.path)
                    .with_identity(ccc_session::FileIdentity::Opaque(
                        resolved.loaded.identity.0.clone(),
                    ))
                    .included_from(parent, Span::new(parent, 0, 0));
                if resolved.system {
                    spec = spec.as_system_header();
                }
                let file = self
                    .session
                    .sources
                    .add_file_occurrence(spec, resolved.loaded.source);
                self.process_file(
                    FileFrame {
                        file,
                        path: resolved.loaded.path,
                        identity: resolved.loaded.identity,
                        found_entry: resolved.entry_index,
                        system: resolved.system,
                        dependency_index,
                        dependency_edge: Some(dependency_edge),
                        is_main: false,
                    },
                    emit_tokens,
                );
            }
            None => self.emit(PpDiagnostic::error(
                "CCC1302",
                format!("cannot open forced input '{}'", path.display()),
            )),
        }
    }

    fn process_file(&mut self, frame: FileFrame, emit_tokens: bool) {
        if self.include_stack.len() >= self.options.limits.include_depth {
            if let Some(cycle_start) = self
                .include_stack
                .iter()
                .rposition(|active| active.identity == frame.identity)
            {
                let mut cycle = self.include_stack[cycle_start..]
                    .iter()
                    .map(|active| active.path.display().to_string())
                    .collect::<Vec<_>>();
                cycle.push(frame.path.display().to_string());
                let mut diagnostic = PpDiagnostic::error(
                    "CCC1335",
                    format!(
                        "include cycle reached the depth limit while entering '{}'",
                        frame.path.display()
                    ),
                )
                .with_note(format!("include cycle: {}", cycle.join(" -> ")));
                if let Some(site) = self
                    .session
                    .sources
                    .file_spec(frame.file)
                    .and_then(|spec| spec.include_site)
                {
                    diagnostic = diagnostic.with_span(site.directive);
                }
                self.emit(diagnostic);
            } else {
                self.emit(PpDiagnostic::error(
                    "CCC1303",
                    format!(
                        "include depth limit exceeded while entering '{}'",
                        frame.path.display()
                    ),
                ));
            }
            return;
        }
        let source = match self.session.sources.source(frame.file) {
            Some(source) => source.to_owned(),
            None => {
                self.emit(PpDiagnostic::error("CCC1304", "unknown source occurrence"));
                return;
            }
        };
        let lexed = match lex_file(
            frame.file,
            &source,
            LexerOptions {
                trigraphs: self.options.trigraphs_enabled(),
                warn_trigraphs: self.options.warn_trigraphs,
            },
        ) {
            Ok(lexed) => lexed,
            Err(error) => {
                self.emit(PpDiagnostic::error(error.code, error.message).with_span(error.span));
                return;
            }
        };
        for diagnostic in lexed.diagnostics {
            self.emit(diagnostic);
        }
        let display_name = self
            .session
            .sources
            .file_name(frame.file)
            .map_or_else(|| frame.path.to_string_lossy().into_owned(), str::to_owned);
        self.include_stack.push(frame.clone());
        if emit_tokens && !self.options.suppress_line_markers {
            let mut flags = Vec::new();
            if !frame.is_main {
                flags.push(1);
            }
            if frame.system {
                flags.push(3);
            }
            self.items.push(PpItem::LineMarker(LineMarker {
                line: 1,
                file: display_name.clone(),
                flags,
            }));
        }

        let mut conditionals = Vec::<ConditionalFrame>::new();
        let mut logical_file = display_name;
        let mut logical_line = 1_usize;
        let mut next_mapping = None::<(usize, Option<String>)>;
        let mut pending_tokens = Vec::<PpToken>::new();
        let mut pending_file = logical_file.clone();
        let mut pending_system = frame.system;
        let mut pending_line_count = 0_usize;
        let mut pending_is_expanded = false;
        let mut pending_can_continue = false;
        for (line_index, (mut line, line_errors)) in
            lexed.lines.into_iter().zip(lexed.line_errors).enumerate()
        {
            let physical_line = line
                .iter()
                .filter_map(|token| {
                    self.session
                        .sources
                        .location(frame.file, token.span.end)
                        .map(|location| location.line)
                })
                .max()
                .unwrap_or(line_index + 1);
            if let Some(location) = line.first().and_then(|token| {
                self.session
                    .sources
                    .presumed_location(frame.file, token.span.start)
            }) {
                logical_line = location.line;
                logical_file = location.file_name.to_owned();
            }
            let system = self.include_stack.last().is_some_and(|frame| frame.system);
            for token in &mut line {
                token.logical_line = self
                    .session
                    .sources
                    .presumed_location(frame.file, token.span.start)
                    .map_or(token.logical_line, |location| location.line);
                token.is_system_header = system;
            }
            if is_active(&conditionals) {
                for error in line_errors {
                    self.emit(PpDiagnostic::error(error.code, error.message).with_span(error.span));
                }
            }
            let directive = directive_parts(&line);
            if let Some((name, operands, hash_span)) = directive {
                if !pending_tokens.is_empty() {
                    let item_count_before_expansion = self.items.len();
                    let tokens = std::mem::take(&mut pending_tokens);
                    if pending_is_expanded {
                        self.process_expanded_line(tokens, emit_tokens);
                    } else {
                        self.expand_ordinary_tokens(
                            tokens,
                            &pending_file,
                            pending_system,
                            emit_tokens,
                        );
                    }
                    pending_is_expanded = false;
                    pending_can_continue = false;
                    self.finish_rendered_lines(
                        item_count_before_expansion,
                        emit_tokens,
                        std::mem::take(&mut pending_line_count),
                    );
                }
                let item_count_before_directive = self.items.len();
                if is_conditional_directive(name) {
                    self.handle_conditional(
                        name,
                        operands,
                        hash_span,
                        &mut conditionals,
                        &logical_file,
                    );
                } else if is_active(&conditionals) {
                    let mapping = self.handle_directive(
                        name,
                        operands,
                        hash_span,
                        physical_line,
                        &logical_file,
                        logical_line,
                        emit_tokens,
                    );
                    if mapping.is_some() {
                        next_mapping = mapping;
                    }
                }
                self.finish_rendered_lines(item_count_before_directive, emit_tokens, 1);
            } else if is_active(&conditionals) {
                if !pending_tokens.is_empty()
                    && !has_unclosed_parenthesis(&pending_tokens)
                    && pending_can_continue
                    && line
                        .first()
                        .is_some_and(|token| token.spelling.as_str() != "(")
                {
                    let item_count_before_expansion = self.items.len();
                    let tokens = std::mem::take(&mut pending_tokens);
                    if pending_is_expanded {
                        self.process_expanded_line(tokens, emit_tokens);
                    } else {
                        self.expand_ordinary_tokens(
                            tokens,
                            &pending_file,
                            pending_system,
                            emit_tokens,
                        );
                    }
                    pending_is_expanded = false;
                    pending_can_continue = false;
                    self.finish_rendered_lines(
                        item_count_before_expansion,
                        emit_tokens,
                        std::mem::take(&mut pending_line_count),
                    );
                }
                if !line.is_empty() {
                    if pending_tokens.is_empty() {
                        pending_file.clone_from(&logical_file);
                        pending_system = system;
                    } else if let Some(first) = line.first_mut() {
                        first.leading_space = true;
                        first.at_start_of_line = false;
                    }
                    pending_tokens.extend(line);
                    pending_is_expanded = false;
                    pending_can_continue = false;
                }
                if pending_tokens.is_empty() {
                    if emit_tokens {
                        self.items.push(PpItem::Newline);
                    }
                } else {
                    pending_line_count = pending_line_count.saturating_add(1);
                    if !has_unclosed_parenthesis(&pending_tokens) {
                        let item_count_before_expansion = self.items.len();
                        let (expanded, can_continue) = if pending_is_expanded {
                            (std::mem::take(&mut pending_tokens), pending_can_continue)
                        } else {
                            self.expand_ordinary_sequence(
                                std::mem::take(&mut pending_tokens),
                                &pending_file,
                                pending_system,
                            )
                        };
                        if can_continue {
                            pending_tokens = expanded;
                            pending_is_expanded = true;
                            pending_can_continue = true;
                        } else {
                            self.process_expanded_line(expanded, emit_tokens);
                            pending_is_expanded = false;
                            pending_can_continue = false;
                            self.finish_rendered_lines(
                                item_count_before_expansion,
                                emit_tokens,
                                std::mem::take(&mut pending_line_count),
                            );
                        }
                    }
                }
            } else if emit_tokens {
                self.items.push(PpItem::Newline);
            }
            if let Some((line, file)) = next_mapping.take() {
                logical_line = line;
                if let Some(file) = file {
                    logical_file = file;
                }
            } else {
                logical_line = logical_line.saturating_add(1);
            }
        }
        if !pending_tokens.is_empty() {
            let item_count_before_expansion = self.items.len();
            if pending_is_expanded {
                self.process_expanded_line(pending_tokens, emit_tokens);
            } else {
                self.expand_ordinary_tokens(
                    pending_tokens,
                    &pending_file,
                    pending_system,
                    emit_tokens,
                );
            }
            self.finish_rendered_lines(
                item_count_before_expansion,
                emit_tokens,
                pending_line_count,
            );
        }
        if let Some(conditional) = conditionals.last() {
            self.emit(
                PpDiagnostic::error("CCC1305", "unterminated conditional directive")
                    .with_span(conditional.opening_span),
            );
        }
        self.include_stack.pop();
    }

    fn expand_ordinary_tokens(
        &mut self,
        tokens: Vec<PpToken>,
        logical_file: &str,
        system: bool,
        emit_tokens: bool,
    ) {
        let (tokens, _) = self.expand_ordinary_sequence(tokens, logical_file, system);
        self.process_expanded_line(tokens, emit_tokens);
    }

    fn expand_ordinary_sequence(
        &mut self,
        tokens: Vec<PpToken>,
        logical_file: &str,
        system: bool,
    ) -> (Vec<PpToken>, bool) {
        if self.options.preprocessed_input {
            return (tokens, false);
        }
        let expansion = expand(
            &mut self.session.sources,
            &mut self.macros,
            &tokens,
            &self.options,
            ExpansionLocation {
                logical_file,
                is_system_header: system,
            },
        );
        let trailing_function_macro_can_continue = expansion.trailing_function_macro_can_continue;
        for diagnostic in expansion.diagnostics {
            self.emit(diagnostic);
        }
        (expansion.tokens, trailing_function_macro_can_continue)
    }

    fn finish_rendered_lines(
        &mut self,
        item_count_before: usize,
        emit_tokens: bool,
        source_line_count: usize,
    ) {
        let item_already_ended_line = self.items.len() > item_count_before
            && match self.items.last() {
                Some(PpItem::LineMarker(_)) => true,
                Some(PpItem::Pragma(
                    PragmaEvent::Once { .. } | PragmaEvent::SystemHeader { .. },
                )) => false,
                Some(PpItem::Pragma(_)) => true,
                _ => false,
            };
        if emit_tokens && !item_already_ended_line {
            self.items.push(PpItem::Newline);
        }
        if emit_tokens {
            self.items.extend(std::iter::repeat_n(
                PpItem::Newline,
                source_line_count.saturating_sub(1),
            ));
        }
    }

    fn handle_conditional(
        &mut self,
        name: &str,
        operands: &[PpToken],
        span: Span,
        stack: &mut Vec<ConditionalFrame>,
        logical_file: &str,
    ) {
        match name {
            "if" | "ifdef" | "ifndef" => {
                let parent_active = is_active(stack);
                let condition = if !parent_active {
                    false
                } else if name == "if" {
                    self.evaluate_condition(operands, logical_file)
                } else {
                    let valid = operands.len() == 1 && operands[0].kind == PpTokenKind::Identifier;
                    if !valid {
                        self.emit(
                            PpDiagnostic::error(
                                "CCC1306",
                                format!("#{name} requires one identifier"),
                            )
                            .with_span(span),
                        );
                        false
                    } else {
                        let defined = self.macros.contains(&operands[0].identifier_key());
                        if name == "ifdef" { defined } else { !defined }
                    }
                };
                stack.push(ConditionalFrame {
                    parent_active,
                    active: parent_active && condition,
                    branch_taken: parent_active && condition,
                    saw_else: false,
                    opening_span: span,
                });
            }
            "elif" => {
                let Some(frame) = stack.last_mut() else {
                    self.emit(
                        PpDiagnostic::error("CCC1307", "#elif without matching #if")
                            .with_span(span),
                    );
                    return;
                };
                if frame.saw_else {
                    self.emit(PpDiagnostic::error("CCC1308", "#elif after #else").with_span(span));
                    frame.active = false;
                    return;
                }
                let should_evaluate = frame.parent_active && !frame.branch_taken;
                let parent_active = frame.parent_active;
                let branch_taken = frame.branch_taken;
                let _ = frame;
                let condition = should_evaluate && self.evaluate_condition(operands, logical_file);
                let frame = stack.last_mut().expect("checked above");
                frame.active = parent_active && !branch_taken && condition;
                frame.branch_taken |= condition;
            }
            "else" => {
                let Some(frame) = stack.last_mut() else {
                    self.emit(
                        PpDiagnostic::error("CCC1309", "#else without matching #if")
                            .with_span(span),
                    );
                    return;
                };
                if !operands.is_empty() {
                    self.emit(PpDiagnostic::error("CCC1310", "tokens after #else").with_span(span));
                }
                if frame.saw_else {
                    self.emit(PpDiagnostic::error("CCC1311", "duplicate #else").with_span(span));
                    frame.active = false;
                } else {
                    frame.active = frame.parent_active && !frame.branch_taken;
                    frame.branch_taken = true;
                    frame.saw_else = true;
                }
            }
            "endif" => {
                if !operands.is_empty() {
                    self.emit(
                        PpDiagnostic::error("CCC1312", "tokens after #endif").with_span(span),
                    );
                }
                if stack.pop().is_none() {
                    self.emit(
                        PpDiagnostic::error("CCC1313", "#endif without matching #if")
                            .with_span(span),
                    );
                }
            }
            _ => unreachable!(),
        }
    }

    fn evaluate_condition(&mut self, operands: &[PpToken], logical_file: &str) -> bool {
        let frame = self.include_stack.last().cloned();
        let entries = self.search_entries.clone();
        let files = self.files;
        let wchar_is_signed = self.session.config.target.data_layout.wchar_is_signed;
        let mut read_failures = Vec::new();
        let result = evaluate_condition(
            &mut self.session.sources,
            &mut self.macros,
            operands,
            &self.options,
            wchar_is_signed,
            ExpansionLocation {
                logical_file,
                is_system_header: frame.as_ref().is_some_and(|frame| frame.system),
            },
            |header, angled, include_next| match resolve_header(
                files,
                &entries,
                frame.as_ref(),
                header,
                angled,
                include_next,
            ) {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(failure) => {
                    read_failures.push((header.to_owned(), failure));
                    false
                }
            },
        );
        for diagnostic in result.diagnostics {
            self.emit(diagnostic);
        }
        for (header, failure) in read_failures {
            let mut diagnostic = PpDiagnostic::error(
                "CCC1334",
                format!(
                    "cannot read header '{header}' at '{}': {}",
                    failure.path.display(),
                    failure.error
                ),
            );
            if let Some(span) = operands.first().map(|token| token.span) {
                diagnostic = diagnostic.with_span(span);
            }
            self.emit(diagnostic);
        }
        result.value
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_directive(
        &mut self,
        name: &str,
        operands: &[PpToken],
        span: Span,
        physical_line: usize,
        logical_file: &str,
        logical_line: usize,
        emit_tokens: bool,
    ) -> Option<(usize, Option<String>)> {
        match name {
            "" => {}
            "define" => self.handle_define(operands),
            "undef" => self.handle_undef(operands, span),
            "include" | "include_next" => self.handle_include(
                operands,
                span,
                name == "include_next",
                logical_file,
                logical_line,
                emit_tokens,
            ),
            "line" => {
                return self.handle_line(operands, span, physical_line, logical_file, emit_tokens);
            }
            "linemarker" => {
                return self.handle_numeric_linemarker(
                    operands,
                    span,
                    physical_line,
                    logical_file,
                    emit_tokens,
                );
            }
            "error" => self
                .emit(PpDiagnostic::error("CCC1314", directive_message(operands)).with_span(span)),
            "warning" => self.emit(
                PpDiagnostic::warning("CCC1315", directive_message(operands)).with_span(span),
            ),
            "pragma" => self.handle_pragma(operands, span, emit_tokens),
            _ => self.emit(
                PpDiagnostic::error(
                    "CCC1316",
                    format!("unknown preprocessing directive '#{name}'"),
                )
                .with_span(span),
            ),
        }
        None
    }

    fn handle_define(&mut self, operands: &[PpToken]) {
        let Some(name_token) = operands
            .first()
            .filter(|token| token.kind == PpTokenKind::Identifier)
        else {
            self.emit(PpDiagnostic::error(
                "CCC1317",
                "#define requires a macro name",
            ));
            return;
        };
        let name = name_token.identifier_key();
        let mut replacement_start = 1;
        let mut form = MacroForm::Object;
        if operands
            .get(1)
            .is_some_and(|token| token.spelling == "(" && !token.leading_space)
        {
            let mut parameters = Vec::new();
            let mut index = 2;
            let mut variadic = false;
            if operands
                .get(index)
                .is_some_and(|token| token.spelling == ")")
            {
                replacement_start = index + 1;
            } else {
                loop {
                    if operands
                        .get(index)
                        .is_some_and(|token| token.spelling == "...")
                    {
                        variadic = true;
                        index += 1;
                        if operands
                            .get(index)
                            .is_none_or(|token| token.spelling != ")")
                        {
                            self.emit(
                                PpDiagnostic::error(
                                    "CCC1318",
                                    "'...' must be the final macro parameter",
                                )
                                .with_span(name_token.span),
                            );
                            return;
                        }
                        replacement_start = index + 1;
                        break;
                    }
                    let Some(parameter) = operands
                        .get(index)
                        .filter(|token| token.kind == PpTokenKind::Identifier)
                    else {
                        self.emit(
                            PpDiagnostic::error("CCC1319", "expected macro parameter name")
                                .with_span(name_token.span),
                        );
                        return;
                    };
                    let parameter = parameter.identifier_key();
                    if parameters.contains(&parameter) {
                        self.emit(
                            PpDiagnostic::error(
                                "CCC1320",
                                format!("duplicate macro parameter '{parameter}'"),
                            )
                            .with_span(name_token.span),
                        );
                        return;
                    }
                    parameters.push(parameter);
                    index += 1;
                    match operands.get(index).map(|token| token.spelling.as_str()) {
                        Some(")") => {
                            replacement_start = index + 1;
                            break;
                        }
                        Some(",") => index += 1,
                        Some("...") if self.options.language_mode == LanguageMode::Gnu11 => {
                            // GNU named variadics use the final parameter name in place of __VA_ARGS__.
                            variadic = true;
                            let named = parameters.pop().expect("just pushed");
                            index += 1;
                            if operands
                                .get(index)
                                .is_none_or(|token| token.spelling != ")")
                            {
                                self.emit(
                                    PpDiagnostic::error(
                                        "CCC1318",
                                        "named variadic parameter must be final",
                                    )
                                    .with_span(name_token.span),
                                );
                                return;
                            }
                            let mut replacement = operands[index + 1..].to_vec();
                            for token in &mut replacement {
                                if token.kind == PpTokenKind::Identifier
                                    && token.identifier_key() == named
                                {
                                    token.spelling = "__VA_ARGS__".to_owned();
                                }
                            }
                            self.install_definition(MacroDefinition {
                                name,
                                form: MacroForm::Function {
                                    parameters,
                                    variadic,
                                },
                                replacement,
                                definition_span: name_token.span,
                                predefined: false,
                            });
                            return;
                        }
                        _ => {
                            self.emit(
                                PpDiagnostic::error(
                                    "CCC1321",
                                    "expected ',' or ')' in macro parameter list",
                                )
                                .with_span(name_token.span),
                            );
                            return;
                        }
                    }
                }
            }
            form = MacroForm::Function {
                parameters,
                variadic,
            };
        }
        self.install_definition(MacroDefinition {
            name,
            form,
            replacement: operands[replacement_start..].to_vec(),
            definition_span: name_token.span,
            predefined: false,
        });
    }

    fn install_definition(&mut self, definition: MacroDefinition) {
        if definition
            .replacement
            .first()
            .is_some_and(|token| matches!(token.spelling.as_str(), "##" | "%:%:"))
            || definition
                .replacement
                .last()
                .is_some_and(|token| matches!(token.spelling.as_str(), "##" | "%:%:"))
        {
            self.emit(
                PpDiagnostic::error(
                    "CCC1107",
                    "'##' cannot appear at either end of a macro replacement",
                )
                .with_span(definition.definition_span),
            );
            return;
        }

        let (parameters, variadic) = match &definition.form {
            MacroForm::Object => (None, false),
            MacroForm::Function {
                parameters,
                variadic,
            } => (Some(parameters.as_slice()), *variadic),
        };
        for (index, token) in definition.replacement.iter().enumerate() {
            if token.kind == PpTokenKind::Identifier && token.identifier_key() == "__VA_OPT__" {
                self.emit(
                    PpDiagnostic::error(
                        "CCC1111",
                        "__VA_OPT__ is not supported by the selected compatibility profile",
                    )
                    .with_span(token.span),
                );
                return;
            }
            if token.kind == PpTokenKind::Identifier
                && token.identifier_key() == "__VA_ARGS__"
                && !variadic
            {
                self.emit(
                    PpDiagnostic::error(
                        "CCC1112",
                        "__VA_ARGS__ may only appear in a variadic macro replacement",
                    )
                    .with_span(token.span),
                );
                return;
            }
            if matches!(token.spelling.as_str(), "#" | "%:")
                && let Some(parameters) = parameters
            {
                let valid_operand = definition.replacement.get(index + 1).is_some_and(|next| {
                    next.kind == PpTokenKind::Identifier
                        && (parameters.contains(&next.identifier_key())
                            || (variadic && next.identifier_key() == "__VA_ARGS__"))
                });
                if !valid_operand {
                    self.emit(
                        PpDiagnostic::error("CCC1106", "'#' must be followed by a macro parameter")
                            .with_span(token.span),
                    );
                    return;
                }
            }
        }
        let name = definition.name.clone();
        let replacement = definition.clone();
        if let DefineResult::Replaced(previous) = self.macros.define(definition) {
            self.emit(redefinition_diagnostic(&replacement, &previous));
        }
        debug_assert!(self.macros.contains(&name));
    }

    fn handle_undef(&mut self, operands: &[PpToken], span: Span) {
        if operands.len() != 1 || operands[0].kind != PpTokenKind::Identifier {
            self.emit(
                PpDiagnostic::error("CCC1322", "#undef requires exactly one identifier")
                    .with_span(span),
            );
            return;
        }
        self.macros.remove(&operands[0].identifier_key());
    }

    fn handle_include(
        &mut self,
        operands: &[PpToken],
        span: Span,
        include_next: bool,
        logical_file: &str,
        logical_line: usize,
        emit_tokens: bool,
    ) {
        let system = self.include_stack.last().is_some_and(|frame| frame.system);
        let expanded_tokens = if parse_header_operand(operands).is_some() {
            operands.to_vec()
        } else {
            let expansion = expand(
                &mut self.session.sources,
                &mut self.macros,
                operands,
                &self.options,
                ExpansionLocation {
                    logical_file,
                    is_system_header: system,
                },
            );
            for diagnostic in expansion.diagnostics {
                self.emit(diagnostic);
            }
            expansion.tokens
        };
        let Some((header, angled)) = parse_header_operand(&expanded_tokens) else {
            self.emit(
                PpDiagnostic::error("CCC1323", "include operand does not form one header name")
                    .with_span(span),
            );
            return;
        };
        let current = self.include_stack.last().cloned();
        let resolved = match resolve_header(
            self.files,
            &self.search_entries,
            current.as_ref(),
            &header,
            angled,
            include_next,
        ) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                self.emit(
                    PpDiagnostic::error("CCC1324", format!("header '{header}' not found"))
                        .with_span(span),
                );
                return;
            }
            Err(failure) => {
                self.emit(
                    PpDiagnostic::error(
                        "CCC1334",
                        format!(
                            "cannot read header '{header}' at '{}': {}",
                            failure.path.display(),
                            failure.error
                        ),
                    )
                    .with_span(span),
                );
                return;
            }
        };
        let parent = current.expect("include directives occur inside a file");
        let child_system = parent.system || resolved.system;
        let dependency_edge = self.dependencies.edges.len();
        self.dependencies.edges.push(DependencyEdge {
            from: parent.path.clone(),
            to: resolved.loaded.path.clone(),
            spelled: header.clone(),
            system: child_system,
        });
        if self.once_files.contains(&resolved.loaded.identity) {
            return;
        }
        let dependency_index = self
            .dependencies
            .record_file(resolved.loaded.path.clone(), child_system);
        let mut spec = SourceFileSpec::new(PathBuf::from(&header))
            .with_resolved_path(&resolved.loaded.path)
            .with_identity(ccc_session::FileIdentity::Opaque(
                resolved.loaded.identity.0.clone(),
            ))
            .included_from(parent.file, span);
        if child_system {
            spec = spec.as_system_header();
        }
        let file = self
            .session
            .sources
            .add_file_occurrence(spec, resolved.loaded.source);
        self.process_file(
            FileFrame {
                file,
                path: resolved.loaded.path,
                identity: resolved.loaded.identity,
                found_entry: resolved.entry_index,
                system: child_system,
                dependency_index,
                dependency_edge: Some(dependency_edge),
                is_main: false,
            },
            emit_tokens,
        );
        if emit_tokens && !self.options.suppress_line_markers {
            self.items.push(PpItem::LineMarker(LineMarker {
                line: logical_line.saturating_add(1),
                file: logical_file.to_owned(),
                flags: if parent.system { vec![2, 3] } else { vec![2] },
            }));
        }
    }

    fn handle_line(
        &mut self,
        operands: &[PpToken],
        span: Span,
        physical_line: usize,
        logical_file: &str,
        emit_tokens: bool,
    ) -> Option<(usize, Option<String>)> {
        let system = self.include_stack.last().is_some_and(|frame| frame.system);
        let expansion = expand(
            &mut self.session.sources,
            &mut self.macros,
            operands,
            &self.options,
            ExpansionLocation {
                logical_file,
                is_system_header: system,
            },
        );
        for diagnostic in expansion.diagnostics {
            self.emit(diagnostic);
        }
        let Some(line_token) = expansion
            .tokens
            .first()
            .filter(|token| token.kind == PpTokenKind::PpNumber)
        else {
            self.emit(
                PpDiagnostic::error("CCC1325", "#line requires a decimal line number")
                    .with_span(span),
            );
            return None;
        };
        let Ok(line) = line_token.spelling.parse::<usize>() else {
            self.emit(
                PpDiagnostic::error("CCC1325", "invalid #line line number")
                    .with_span(line_token.span),
            );
            return None;
        };
        if line == 0 || line > 2_147_483_647 {
            self.emit(
                PpDiagnostic::error("CCC1325", "#line line number is out of range")
                    .with_span(line_token.span),
            );
            return None;
        }
        let file = match expansion.tokens.get(1) {
            None => None,
            Some(token) if token.kind == PpTokenKind::StringLiteral => token
                .spelling
                .strip_prefix('"')
                .and_then(|body| body.strip_suffix('"'))
                .map(str::to_owned),
            Some(token) => {
                self.emit(
                    PpDiagnostic::error("CCC1326", "#line file name must be a string literal")
                        .with_span(token.span),
                );
                return None;
            }
        };
        if expansion.tokens.len() > 2 {
            self.emit(
                PpDiagnostic::error("CCC1327", "trailing tokens after #line")
                    .with_span(expansion.tokens[2].span),
            );
            return None;
        }
        let current_file = self.include_stack.last().expect("inside file").file;
        let _ = self.session.sources.add_line_mapping(
            current_file,
            physical_line.saturating_add(1),
            line,
            file.clone(),
        );
        if emit_tokens && !self.options.suppress_line_markers {
            self.items.push(PpItem::LineMarker(LineMarker {
                line,
                file: file.clone().unwrap_or_else(|| logical_file.to_owned()),
                flags: if system { vec![3] } else { Vec::new() },
            }));
        }
        Some((line, file))
    }

    fn handle_numeric_linemarker(
        &mut self,
        operands: &[PpToken],
        span: Span,
        physical_line: usize,
        logical_file: &str,
        emit_tokens: bool,
    ) -> Option<(usize, Option<String>)> {
        let Some(line_token) = operands
            .first()
            .filter(|token| token.kind == PpTokenKind::PpNumber)
        else {
            self.emit(
                PpDiagnostic::error("CCC1331", "linemarker requires a line number").with_span(span),
            );
            return None;
        };
        let Ok(line) = line_token.spelling.parse::<usize>() else {
            self.emit(
                PpDiagnostic::error("CCC1331", "invalid linemarker line number")
                    .with_span(line_token.span),
            );
            return None;
        };
        if line == 0 || line > 2_147_483_647 {
            self.emit(
                PpDiagnostic::error("CCC1331", "linemarker line number is out of range")
                    .with_span(line_token.span),
            );
            return None;
        }
        let mut index = 1;
        let file = operands.get(index).and_then(|token| {
            (token.kind == PpTokenKind::StringLiteral).then(|| {
                token
                    .spelling
                    .strip_prefix('"')
                    .and_then(|body| body.strip_suffix('"'))
                    .unwrap_or(&token.spelling)
                    .to_owned()
            })
        });
        if file.is_some() {
            index += 1;
        }
        let mut flags = Vec::new();
        for token in &operands[index..] {
            let Ok(flag) = token.spelling.parse::<u8>() else {
                self.emit(
                    PpDiagnostic::error("CCC1332", "invalid linemarker flag").with_span(token.span),
                );
                return None;
            };
            if !(1..=4).contains(&flag) {
                self.emit(
                    PpDiagnostic::error("CCC1332", "linemarker flag must be between 1 and 4")
                        .with_span(token.span),
                );
                return None;
            }
            flags.push(flag);
        }
        let current_file = self.include_stack.last().expect("inside file").file;
        let _ = self.session.sources.add_line_mapping(
            current_file,
            physical_line.saturating_add(1),
            line,
            file.clone(),
        );
        let system = flags.contains(&3);
        let offset = operands.last().map_or(span.end, |token| token.span.end);
        let _ = self
            .session
            .sources
            .set_system_header_from(current_file, offset, system);
        self.set_current_file_system(system);
        if emit_tokens && !self.options.suppress_line_markers {
            self.items.push(PpItem::LineMarker(LineMarker {
                line,
                file: file.clone().unwrap_or_else(|| logical_file.to_owned()),
                flags,
            }));
        }
        Some((line, file))
    }

    fn handle_pragma(&mut self, operands: &[PpToken], span: Span, emit_tokens: bool) {
        let text = directive_message(operands);
        let event = if operands
            .first()
            .is_some_and(|token| token.spelling == "once")
        {
            if let Some(frame) = self.include_stack.last() {
                self.once_files.insert(frame.identity.clone());
            }
            PragmaEvent::Once { span }
        } else if spellings_start_with(operands, &["GCC", "system_header"]) {
            let frame = self.include_stack.last().cloned();
            if frame.as_ref().is_some_and(|frame| frame.is_main) {
                self.emit(
                    PpDiagnostic::warning(
                        "CCC1333",
                        "#pragma GCC system_header is ignored in the main file",
                    )
                    .with_span(span)
                    .with_category(PpDiagnosticCategory::SystemHeader),
                );
            } else if let Some(frame) = frame {
                let offset = operands.last().map_or(span.end, |token| token.span.end);
                let _ = self
                    .session
                    .sources
                    .set_system_header_from(frame.file, offset, true);
                self.set_current_file_system(true);
            }
            PragmaEvent::SystemHeader { span }
        } else if spellings_start_with(operands, &["GCC", "diagnostic"]) {
            let action = match operands.get(2).map(|token| token.spelling.as_str()) {
                Some("push") => Some(DiagnosticPragmaAction::Push),
                Some("pop") => Some(DiagnosticPragmaAction::Pop),
                Some("ignored") => Some(DiagnosticPragmaAction::Ignored),
                Some("warning") => Some(DiagnosticPragmaAction::Warning),
                Some("error") => Some(DiagnosticPragmaAction::Error),
                _ => None,
            };
            let Some(action) = action else {
                self.emit(
                    PpDiagnostic::warning("CCC1328", "unknown GCC diagnostic pragma")
                        .with_span(span),
                );
                return;
            };
            let option = operands.get(3).and_then(|token| {
                token
                    .spelling
                    .strip_prefix('"')?
                    .strip_suffix('"')
                    .map(str::to_owned)
            });
            PragmaEvent::Diagnostic {
                action,
                option,
                span,
            }
        } else if spellings_start_with(operands, &["GCC", "optimize"]) {
            PragmaEvent::GccOptimize {
                payload: operands[2..].to_vec(),
                span,
            }
        } else if operands
            .first()
            .is_some_and(|token| token.spelling == "pack")
        {
            PragmaEvent::Pack {
                payload: operands[1..].to_vec(),
                span,
            }
        } else {
            let system = self.include_stack.last().is_some_and(|frame| frame.system);
            self.emit(
                PpDiagnostic::new(PpSeverity::Warning, "CCC1329", "unknown pragma")
                    .with_span(span)
                    .with_category(PpDiagnosticCategory::UnknownPragma)
                    .in_system_header(system),
            );
            PragmaEvent::Unknown { text, span }
        };
        let system_marker = if matches!(event, PragmaEvent::SystemHeader { .. })
            && self
                .include_stack
                .last()
                .is_some_and(|frame| !frame.is_main)
        {
            self.include_stack.last().and_then(|frame| {
                self.session
                    .sources
                    .presumed_location(frame.file, span.end)
                    .map(|location| LineMarker {
                        line: location.line.saturating_add(1),
                        file: location.file_name.to_owned(),
                        flags: vec![3],
                    })
            })
        } else {
            None
        };
        self.diagnostics.handle_pragma(&event);
        if emit_tokens {
            self.items.push(PpItem::Pragma(event));
            if !self.options.suppress_line_markers
                && let Some(marker) = system_marker
            {
                self.items.push(PpItem::LineMarker(marker));
            }
        }
    }

    fn set_current_file_system(&mut self, system: bool) {
        let Some(frame) = self.include_stack.last_mut() else {
            return;
        };
        frame.system = system;
        if let Some(dependency) = self.dependencies.files.get_mut(frame.dependency_index) {
            dependency.system = system;
        }
        if let Some(edge) = frame
            .dependency_edge
            .and_then(|index| self.dependencies.edges.get_mut(index))
        {
            edge.system = system;
        }
    }

    fn process_expanded_line(&mut self, tokens: Vec<PpToken>, emit_tokens: bool) {
        let (tokens, mut pragmas, diagnostics) = parse_pragma_operators(tokens);
        for diagnostic in diagnostics {
            self.emit(diagnostic);
        }
        pragmas.sort_by_key(|(position, _, _)| *position);
        let mut pragma_index = 0;
        for (position, token) in tokens.into_iter().enumerate() {
            let mut handled_pragma = false;
            while pragmas
                .get(pragma_index)
                .is_some_and(|pragma| pragma.0 == position)
            {
                let (_, text, span) = pragmas[pragma_index].clone();
                self.handle_pragma_text(&text, span, emit_tokens);
                pragma_index += 1;
                handled_pragma = true;
            }
            if handled_pragma && emit_tokens && !self.options.suppress_line_markers {
                let location = self
                    .session
                    .sources
                    .presumed_location(token.span.file, token.span.start);
                if let Some(location) = location {
                    self.items.push(PpItem::LineMarker(LineMarker {
                        line: location.line,
                        file: location.file_name.to_owned(),
                        flags: self
                            .include_stack
                            .last()
                            .is_some_and(|frame| frame.system)
                            .then_some(3)
                            .into_iter()
                            .collect(),
                    }));
                }
            }
            if emit_tokens {
                self.items.push(PpItem::Token(token));
            }
        }
        while let Some((_, text, span)) = pragmas.get(pragma_index).cloned() {
            self.handle_pragma_text(&text, span, emit_tokens);
            pragma_index += 1;
        }
    }

    fn handle_pragma_text(&mut self, text: &str, span: Span, emit_tokens: bool) {
        let operands = match lex_fragment(span, text) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.emit(PpDiagnostic::error(error.code, error.message).with_span(span));
                return;
            }
        };
        self.handle_pragma(&operands, span, emit_tokens);
    }

    fn emit(&mut self, mut diagnostic: PpDiagnostic) {
        if let Some(span) = diagnostic.span {
            diagnostic.is_system_header |= self.session.sources.is_system_header(span);
        }
        if self.options.dependency_mode == crate::options::DependencyMode::All
            && diagnostic.severity == PpSeverity::Warning
        {
            return;
        }
        if self.diagnostic_count >= self.options.limits.diagnostics {
            if !self.diagnostic_limit_reported {
                self.diagnostic_limit_reported = true;
                self.error_count += 1;
                self.diagnostics.emit(PpDiagnostic::error(
                    "CCC1399",
                    "too many preprocessing diagnostics",
                ));
            }
            return;
        }
        self.diagnostic_count += 1;
        if diagnostic.severity == PpSeverity::Error {
            self.error_count += 1;
        }
        self.diagnostics.emit(diagnostic);
    }
}

fn has_unclosed_parenthesis(tokens: &[PpToken]) -> bool {
    let mut depth = 0_usize;
    for token in tokens {
        match token.spelling.as_str() {
            "(" => depth = depth.saturating_add(1),
            ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth != 0
}

#[derive(Clone, Debug)]
struct ConditionalFrame {
    parent_active: bool,
    active: bool,
    branch_taken: bool,
    saw_else: bool,
    opening_span: Span,
}

fn is_active(stack: &[ConditionalFrame]) -> bool {
    stack.last().is_none_or(|frame| frame.active)
}

fn is_conditional_directive(name: &str) -> bool {
    matches!(name, "if" | "ifdef" | "ifndef" | "elif" | "else" | "endif")
}

fn directive_parts(tokens: &[PpToken]) -> Option<(&str, &[PpToken], Span)> {
    let hash = tokens
        .first()
        .filter(|token| matches!(token.spelling.as_str(), "#" | "%:"))?;
    if tokens.len() == 1 {
        return Some(("", &[], hash.span));
    }
    if tokens[1].kind == PpTokenKind::PpNumber {
        return Some(("linemarker", &tokens[1..], hash.span));
    }
    let Some(name) = tokens
        .get(1)
        .filter(|token| token.kind == PpTokenKind::Identifier)
    else {
        return Some(("<invalid>", &tokens[1..], hash.span));
    };
    Some((name.spelling.as_str(), &tokens[2..], hash.span))
}

fn directive_message(tokens: &[PpToken]) -> String {
    let mut output = String::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&token.spelling);
    }
    output
}

fn spellings_start_with(tokens: &[PpToken], expected: &[&str]) -> bool {
    tokens.len() >= expected.len()
        && tokens
            .iter()
            .zip(expected)
            .all(|(token, expected)| token.spelling == *expected)
}

struct ResolvedHeader {
    loaded: LoadedFile,
    entry_index: Option<usize>,
    system: bool,
}

#[derive(Debug)]
struct HeaderReadFailure {
    path: PathBuf,
    error: io::Error,
}

fn read_header_candidate(
    files: &dyn FileProvider,
    path: &Path,
) -> Result<Option<LoadedFile>, HeaderReadFailure> {
    match files.read(path) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HeaderReadFailure {
            path: path.to_path_buf(),
            error,
        }),
    }
}

fn resolve_header(
    files: &dyn FileProvider,
    entries: &[SearchEntry],
    current: Option<&FileFrame>,
    header: &str,
    angled: bool,
    include_next: bool,
) -> Result<Option<ResolvedHeader>, HeaderReadFailure> {
    let header_path = Path::new(header);
    if header_path.is_absolute() {
        return read_header_candidate(files, header_path).map(|loaded| {
            loaded.map(|loaded| ResolvedHeader {
                loaded,
                entry_index: None,
                system: false,
            })
        });
    }
    if !angled
        && !include_next
        && let Some(parent) = current.and_then(|frame| frame.path.parent())
    {
        let candidate = parent.join(header_path);
        if let Some(loaded) = read_header_candidate(files, &candidate)? {
            return Ok(Some(ResolvedHeader {
                loaded,
                entry_index: None,
                system: current.is_some_and(|frame| frame.system),
            }));
        }
    }
    let start = if include_next {
        current
            .and_then(|frame| frame.found_entry)
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    for (index, entry) in entries
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, entry)| !angled || !entry.quote_only)
    {
        let candidate = entry.path.join(header_path);
        if let Some(loaded) = read_header_candidate(files, &candidate)? {
            return Ok(Some(ResolvedHeader {
                loaded,
                entry_index: Some(index),
                system: entry.system,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;

    use ccc_session::Session;

    use super::*;
    use crate::diagnostic::VecDiagnosticSink;
    use crate::files::LoadedFile;
    use crate::render::render_preprocessed;

    #[derive(Default)]
    struct MemoryFiles(BTreeMap<PathBuf, String>);

    impl FileProvider for MemoryFiles {
        fn read(&self, path: &Path) -> io::Result<LoadedFile> {
            let path = path.to_path_buf();
            let source = self
                .0
                .get(&path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not found"))?;
            Ok(LoadedFile {
                identity: FileIdentity(path.to_string_lossy().into_owned()),
                path,
                source,
            })
        }
    }

    #[derive(Default)]
    struct FailingFiles {
        files: MemoryFiles,
        failures: BTreeMap<PathBuf, io::ErrorKind>,
    }

    impl FileProvider for FailingFiles {
        fn read(&self, path: &Path) -> io::Result<LoadedFile> {
            if let Some(kind) = self.failures.get(path) {
                return Err(io::Error::new(*kind, "injected read failure"));
            }
            self.files.read(path)
        }
    }

    struct SharedIdentityFiles(MemoryFiles);

    impl FileProvider for SharedIdentityFiles {
        fn read(&self, path: &Path) -> io::Result<LoadedFile> {
            let mut loaded = self.0.read(path)?;
            loaded.identity = FileIdentity("shared-physical-file".to_owned());
            Ok(loaded)
        }
    }

    fn run(
        source: &str,
        files: &dyn FileProvider,
        options: &PreprocessOptions,
    ) -> (PreprocessOutput, VecDiagnosticSink) {
        let mut session = Session::default();
        let file = session.sources.add_file("/project/main.c", source);
        let mut diagnostics = VecDiagnosticSink::default();
        let output = preprocess(
            &mut PreprocessContext {
                session: &mut session,
                diagnostics: &mut diagnostics,
                options,
                files,
            },
            file,
        );
        (output, diagnostics)
    }

    #[test]
    fn expands_macros_and_conditionals() {
        let source = "#define VALUE 40\n#define ADD(x,y) ((x)+(y))\n#if VALUE == 40\nADD(VALUE, 2)\n#else\n0\n#endif\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(render_preprocessed(&output, true).contains("((40)+(2))"));
    }

    #[test]
    fn evaluates_full_prefixed_character_values_in_conditions() {
        let source = "#if L'中' == 20013 && u'中' == 20013 && U'中' == 20013\nmatched\n#else\nunmatched\n#endif\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();
        assert!(spellings.contains(&"matched"));
        assert!(!spellings.contains(&"unmatched"));
    }

    #[test]
    fn applies_prefixed_character_signedness_in_conditions() {
        let source = concat!(
            r"#if U'\U0010FFFF' > -1 || u'\uFFFF' > -1",
            "\nunsigned_was_treated_as_signed\n",
            r"#elif L'\U0010FFFF' > -1",
            "\nmatched\n#endif\n",
        );
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );

        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        let spellings = tokens
            .iter()
            .map(|token| token.spelling.as_str())
            .collect::<Vec<_>>();
        assert_eq!(spellings, ["matched"]);
    }

    #[test]
    fn diagnoses_invalid_prefixed_character_constants_in_conditions() {
        for (source, code) in [
            ("#if u8'a'\nmatched\n#endif\n", "CCC0005"),
            (r"#if L'\u0041'\nmatched\n#endif\n", "CCC0006"),
        ] {
            let source = source.replace("\\n", "\n");
            let (output, diagnostics) = run(
                &source,
                &MemoryFiles::default(),
                &PreprocessOptions::default(),
            );
            assert!(output.had_errors, "{source}");
            assert!(
                diagnostics
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == code),
                "{source}: {:#?}",
                diagnostics.diagnostics
            );
        }
    }

    #[test]
    fn keeps_multiline_comments_inside_directive_lines() {
        let source = "#define X /* first\nsecond */ 1\n#if X == 1\nmatched\n#endif\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(
            output
                .tokens()
                .iter()
                .any(|token| token.spelling == "matched")
        );
    }

    #[test]
    fn preprocesses_catch_all_tokens_and_dollar_identifiers() {
        let source = "#define S(x) #x\n#define $value 42\nS(a\\b)\n@ ` $value\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        let spellings: Vec<_> = tokens.iter().map(|token| token.spelling.as_str()).collect();
        assert_eq!(spellings, [r#""a\b""#, "@", "`", "42"]);
    }

    #[test]
    fn accepts_vertical_tabs_as_directive_whitespace() {
        let source = "#define\u{000b}X\u{000b}42\nX\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].spelling, "42");
    }

    #[test]
    fn resolves_includes_and_honors_pragma_once() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/project/header.h"),
            "#pragma once\n#define ANSWER 42\n".to_owned(),
        );
        let source = "#include \"header.h\"\n#include \"header.h\"\nANSWER\n";
        let (output, diagnostics) = run(
            source,
            &files,
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(render_preprocessed(&output, true).contains("42"));
        assert_eq!(output.dependencies.files.len(), 2);
    }

    #[test]
    fn does_not_expand_tokens_inside_a_direct_header_name() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/includes/x.h"),
            "#define DIRECT 1\n".to_owned(),
        );
        files.0.insert(
            PathBuf::from("/includes/replaced.h"),
            "#define WRONG 1\n".to_owned(),
        );
        let source = "#define x replaced\n#include <x.h>\nDIRECT\n";
        let (output, diagnostics) = run(
            source,
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/includes",
                    IncludePathKind::User,
                )],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "1"));
    }

    #[test]
    fn searches_explicit_system_paths_before_resource_paths() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/isystem/pick.h"),
            "#define PICKED 1\n".to_owned(),
        );
        files.0.insert(
            PathBuf::from("/resource/pick.h"),
            "#define PICKED 2\n".to_owned(),
        );
        let (output, diagnostics) = run(
            "#include <pick.h>\nPICKED\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![
                    crate::options::IncludePath::new("/resource", IncludePathKind::Resource),
                    crate::options::IncludePath::new("/isystem", IncludePathKind::System),
                ],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "1"));
        assert!(!output.tokens().iter().any(|token| token.spelling == "2"));
    }

    #[test]
    fn searches_quote_only_paths_only_for_quoted_includes() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/quotes/pick.h"),
            "#define QUOTED_PICK 1\n".to_owned(),
        );
        files.0.insert(
            PathBuf::from("/includes/pick.h"),
            "#define ANGLED_PICK 1\n".to_owned(),
        );
        let (output, diagnostics) = run(
            "#include \"pick.h\"\n#include <pick.h>\nQUOTED_PICK ANGLED_PICK\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![
                    crate::options::IncludePath::new("/quotes", IncludePathKind::Quote),
                    crate::options::IncludePath::new("/includes", IncludePathKind::User),
                ],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["1", "1"]
        );
        assert_eq!(
            output.dependencies.edges[0].to,
            PathBuf::from("/quotes/pick.h")
        );
        assert_eq!(
            output.dependencies.edges[1].to,
            PathBuf::from("/includes/pick.h")
        );
    }

    #[test]
    fn duplicate_search_entries_do_not_intercept_include_next() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/wrapper/assert.h"),
            concat!(
                "#ifndef WRAPPER_ASSERT_H\n",
                "#define WRAPPER_ASSERT_H\n",
                "#include_next <assert.h>\n",
                "#endif\n",
            )
            .to_owned(),
        );
        files.0.insert(
            PathBuf::from("/system/assert.h"),
            "#define ASSERT_HEADER_REACHED 42\n".to_owned(),
        );

        let (output, diagnostics) = run(
            "#include <assert.h>\nASSERT_HEADER_REACHED\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![
                    crate::options::IncludePath::new("/wrapper", IncludePathKind::User),
                    crate::options::IncludePath::new("/unrelated", IncludePathKind::User),
                    crate::options::IncludePath::new("/wrapper", IncludePathKind::User),
                    crate::options::IncludePath::new("/system", IncludePathKind::User),
                ],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
        assert_eq!(output.dependencies.edges.len(), 2);
        assert_eq!(
            output.dependencies.edges[1].to,
            PathBuf::from("/system/assert.h")
        );
    }

    #[test]
    fn searches_include_paths_for_forced_inputs() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/includes/forced.h"),
            "#define FORCED 42\n".to_owned(),
        );
        let (output, diagnostics) = run(
            "FORCED\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/includes",
                    IncludePathKind::User,
                )],
                forced_includes: vec![PathBuf::from("forced.h")],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "42"));
        assert_eq!(output.dependencies.edges[0].spelled, "forced.h");
    }

    #[test]
    fn a_fatal_header_read_does_not_continue_the_search() {
        let mut files = FailingFiles::default();
        files.failures.insert(
            PathBuf::from("/unreadable/target.h"),
            io::ErrorKind::PermissionDenied,
        );
        files.files.0.insert(
            PathBuf::from("/fallback/target.h"),
            "#define FALLBACK_VALUE 42\n".to_owned(),
        );

        let (output, diagnostics) = run(
            "#include <target.h>\nFALLBACK_VALUE\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![
                    crate::options::IncludePath::new("/unreadable", IncludePathKind::User),
                    crate::options::IncludePath::new("/fallback", IncludePathKind::User),
                ],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(output.had_errors);
        let diagnostic = diagnostics
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CCC1334")
            .expect("fatal header read diagnostic");
        assert!(diagnostic.message.contains("/unreadable/target.h"));
        assert!(!output.tokens().iter().any(|token| token.spelling == "42"));
    }

    #[test]
    fn a_fatal_forced_input_read_does_not_fall_back_to_search_paths() {
        let mut files = FailingFiles::default();
        files
            .failures
            .insert(PathBuf::from("forced.h"), io::ErrorKind::PermissionDenied);
        files.files.0.insert(
            PathBuf::from("/fallback/forced.h"),
            "#define FORCED_VALUE 42\n".to_owned(),
        );

        let (output, diagnostics) = run(
            "FORCED_VALUE\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/fallback",
                    IncludePathKind::User,
                )],
                forced_includes: vec![PathBuf::from("forced.h")],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(output.had_errors);
        let diagnostic = diagnostics
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CCC1302")
            .expect("fatal forced-input read diagnostic");
        assert!(diagnostic.message.contains("injected read failure"));
        assert!(!output.tokens().iter().any(|token| token.spelling == "42"));
    }

    #[test]
    fn has_include_reports_fatal_candidate_reads() {
        let mut files = FailingFiles::default();
        files.failures.insert(
            PathBuf::from("/unreadable/target.h"),
            io::ErrorKind::InvalidData,
        );

        let (output, diagnostics) = run(
            "#if __has_include(<target.h>)\nint found;\n#endif\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/unreadable",
                    IncludePathKind::User,
                )],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(output.had_errors);
        assert!(
            diagnostics
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CCC1334")
        );
    }

    #[test]
    fn direct_has_include_operands_are_not_macro_expanded_but_computed_ones_are() {
        let mut files = MemoryFiles::default();
        files
            .0
            .insert(PathBuf::from("/includes/stddef.h"), String::new());
        files
            .0
            .insert(PathBuf::from("/includes/computed.h"), String::new());

        let (output, diagnostics) = run(
            concat!(
                "#define stddef replaced\n",
                "#define HEADER <computed.h>\n",
                "#if __has_include(<stddef.h>)\n",
                "#define DIRECT 1\n",
                "#endif\n",
                "#if __has_include(HEADER)\n",
                "#define COMPUTED 2\n",
                "#endif\n",
                "DIRECT COMPUTED\n",
            ),
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/includes",
                    IncludePathKind::User,
                )],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
    }

    #[test]
    fn has_include_next_uses_the_current_search_entry() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/first/probe.h"),
            concat!(
                "#define COMPUTED_NEXT <only-next.h>\n",
                "#if __has_include(<only-first.h>)\n",
                "#define FOUND_FIRST 1\n",
                "#endif\n",
                "#if !__has_include_next(<only-first.h>)\n",
                "#define SKIPPED_FIRST 2\n",
                "#endif\n",
                "#if __has_include_next(COMPUTED_NEXT)\n",
                "#define FOUND_NEXT 3\n",
                "#endif\n",
            )
            .to_owned(),
        );
        files
            .0
            .insert(PathBuf::from("/first/only-first.h"), String::new());
        files
            .0
            .insert(PathBuf::from("/second/only-next.h"), String::new());

        let (output, diagnostics) = run(
            "#include <probe.h>\nFOUND_FIRST SKIPPED_FIRST FOUND_NEXT\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![
                    crate::options::IncludePath::new("/first", IncludePathKind::User),
                    crate::options::IncludePath::new("/second", IncludePathKind::User),
                ],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
    }

    #[test]
    fn macro_controlled_self_inclusion_can_terminate() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/project/recursive.h"),
            concat!(
                "#ifndef LEVEL\n",
                "#define LEVEL 0\n",
                "#endif\n",
                "#if LEVEL == 0\n",
                "#undef LEVEL\n",
                "#define LEVEL 1\n",
                "#include \"recursive.h\"\n",
                "#elif LEVEL == 1\n",
                "#undef LEVEL\n",
                "#define LEVEL 2\n",
                "#include \"recursive.h\"\n",
                "#else\n",
                "#define RECURSION_FINISHED 42\n",
                "#endif\n",
            )
            .to_owned(),
        );

        let (output, diagnostics) = run(
            "#include \"recursive.h\"\nRECURSION_FINISHED\n",
            &files,
            &PreprocessOptions {
                limits: crate::options::PreprocessLimits {
                    include_depth: 4,
                    ..crate::options::PreprocessLimits::default()
                },
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
    }

    #[test]
    fn reports_an_identity_aware_cycle_at_the_include_depth_limit() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/project/first.h"),
            "#include \"alias.h\"\n".to_owned(),
        );
        files.0.insert(
            PathBuf::from("/project/alias.h"),
            "#include \"first.h\"\n".to_owned(),
        );
        let files = SharedIdentityFiles(files);

        let (output, diagnostics) = run(
            "#include \"first.h\"\n",
            &files,
            &PreprocessOptions {
                limits: crate::options::PreprocessLimits {
                    include_depth: 3,
                    ..crate::options::PreprocessLimits::default()
                },
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );

        assert!(output.had_errors);
        let diagnostic = diagnostics
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CCC1335")
            .expect("include-cycle diagnostic");
        assert!(diagnostic.message.contains("depth limit"));
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("alias.h") && note.contains("first.h"))
        );
        assert_eq!(output.dependencies.edges.len(), 3);
    }

    #[test]
    fn does_not_open_includes_in_inactive_groups() {
        let source = "#if 0\n#include \"missing.h\"\n#endif\n42\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "42"));
    }

    #[test]
    fn tolerates_malformed_tokens_in_inactive_groups() {
        let source = "#if 0\nthis isn't C @ all\n#endif\n42\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "42"));
    }

    #[test]
    fn system_header_pragma_updates_the_include_occurrence() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/includes/system.h"),
            "#pragma GCC system_header\n#pragma unknown\nint value;\n".to_owned(),
        );
        let (output, diagnostics) = run(
            "#include <system.h>\n",
            &files,
            &PreprocessOptions {
                include_paths: vec![crate::options::IncludePath::new(
                    "/includes",
                    IncludePathKind::User,
                )],
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.dependencies.files[1].system);
        assert!(output.dependencies.edges[0].system);
        assert!(output.tokens().iter().all(|token| token.is_system_header));
        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert!(diagnostics.diagnostics[0].is_system_header);
    }

    #[test]
    fn system_header_pragma_does_not_reclassify_the_main_file() {
        let (output, diagnostics) = run(
            "#pragma GCC system_header\nint value;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(!output.dependencies.files[0].system);
        assert!(output.tokens().iter().all(|token| !token.is_system_header));
        assert!(
            diagnostics
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CCC1333")
        );
    }

    #[test]
    fn expands_function_invocations_across_newlines() {
        let source = "#define ID(x) x\nID(\n42\n)\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
    }

    #[test]
    fn expands_function_macro_when_newline_precedes_open_parenthesis() {
        let source = "#define ID(x) x\nID\n(\n42\n)\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
        assert!(!tokens[0].span.origin.is_direct());
    }

    #[test]
    fn rescans_a_function_macro_result_with_a_next_line_invocation() {
        let source = "#define ID(x) x\n#define F() ID\nF()\n(42)\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
    }

    #[test]
    fn does_not_rescan_backward_across_an_empty_macro() {
        let source = "#define ID(x) x\n#define F ID\n#define EMPTY\nF EMPTY\n(42)\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(
            output
                .tokens()
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>(),
            ["ID", "(", "42", ")"]
        );
    }

    #[test]
    fn trailing_function_macro_does_not_consume_an_unrelated_line() {
        let source = "#define ID(x) x\nID\nint value;\n";
        let (output, diagnostics) = run(
            source,
            &MemoryFiles::default(),
            &PreprocessOptions {
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert_eq!(render_preprocessed(&output, true), "\nID\nint value;\n\n");
    }

    #[test]
    fn line_macro_uses_the_physical_line_after_a_splice() {
        let (output, diagnostics) = run(
            "\\\n__LINE__\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(output.tokens().iter().any(|token| token.spelling == "2"));
    }

    #[test]
    fn rendered_output_preserves_lines_consumed_by_directives_and_blanks() {
        let (output, diagnostics) = run(
            "#define HIDDEN 1\n\nint value;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let rendered = render_preprocessed(&output, false);
        assert_eq!(
            rendered
                .lines()
                .position(|line| line.contains("int value;")),
            Some(3),
            "{rendered}"
        );
    }

    #[test]
    fn rendered_output_restores_lines_consumed_by_buffered_tokens() {
        let (output, diagnostics) = run(
            "int x = (\n1 +\n2);\nint y;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let rendered = render_preprocessed(&output, false);
        assert_eq!(
            rendered.lines().position(|line| line.contains("int y;")),
            Some(4),
            "{rendered}"
        );

        let (reprocessed, reprocess_diagnostics) = run(
            &rendered,
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(
            !reprocessed.had_errors,
            "{:#?}",
            reprocess_diagnostics.diagnostics
        );
        assert_eq!(
            reprocessed
                .tokens()
                .iter()
                .find(|token| token.spelling == "y")
                .map(|token| token.logical_line),
            Some(4)
        );
    }

    #[test]
    fn rendered_output_restores_location_after_line_and_pragma_events() {
        let (line_output, line_diagnostics) = run(
            "#line 80 \"generated.c\"\nint line_value;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(
            !line_output.had_errors,
            "{:#?}",
            line_diagnostics.diagnostics
        );
        let rendered_line = render_preprocessed(&line_output, false);
        let line_lines = rendered_line.lines().collect::<Vec<_>>();
        let marker = line_lines
            .iter()
            .position(|line| *line == "# 80 \"generated.c\"")
            .unwrap();
        assert_eq!(line_lines[marker + 1], "int line_value;");

        let (pragma_output, pragma_diagnostics) = run(
            "#define DO_PRAGMA _Pragma(\"pack(1)\")\nDO_PRAGMA int pragma_value;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(
            !pragma_output.had_errors,
            "{:#?}",
            pragma_diagnostics.diagnostics
        );
        let rendered_pragma = render_preprocessed(&pragma_output, false);
        let pragma_lines = rendered_pragma.lines().collect::<Vec<_>>();
        let pragma = pragma_lines
            .iter()
            .position(|line| line.starts_with("#pragma pack"))
            .unwrap();
        assert!(pragma_lines[pragma + 1].starts_with("# 2 \"/project/main.c\""));
        assert_eq!(pragma_lines[pragma + 2], "int pragma_value;");
    }

    #[test]
    fn recognizes_gcc_optimize_pragma_operators_without_an_unknown_warning() {
        let (output, diagnostics) = run(
            "int _Pragma(\"GCC optimize(\\\"no-tree-vectorize\\\")\") value;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        assert!(diagnostics.diagnostics.is_empty());
        assert!(output.items.iter().any(|item| matches!(
            item,
            PpItem::Pragma(PragmaEvent::GccOptimize { payload, .. })
                if payload.iter().map(|token| token.spelling.as_str()).collect::<Vec<_>>()
                    == ["(", "\"no-tree-vectorize\"", ")"]
        )));
    }

    #[test]
    fn consumed_pragmas_retain_their_source_line() {
        let (output, diagnostics) = run(
            "#pragma once\nint line_value = __LINE__;\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let rendered = render_preprocessed(&output, false);
        let lines = rendered.lines().collect::<Vec<_>>();
        let marker = lines
            .iter()
            .position(|line| line.starts_with("# 1 \"/project/main.c\""))
            .unwrap();
        assert_eq!(lines[marker + 1], "");
        assert_eq!(lines[marker + 2], "int line_value = 2;");
    }

    #[test]
    fn accepts_numeric_linemarkers_from_preprocessed_input() {
        let (output, diagnostics) = run(
            "# 12 \"logical.c\" 3\n__LINE__ __FILE__\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let tokens = output.tokens();
        assert_eq!(tokens[0].spelling, "12");
        assert_eq!(tokens[1].spelling, "\"logical.c\"");
        assert!(tokens.iter().all(|token| token.is_system_header));
    }

    #[test]
    fn preprocessed_input_forwards_tokens_without_reapplying_macros_or_forced_inputs() {
        let mut files = MemoryFiles::default();
        files.0.insert(
            PathBuf::from("/project/forced.h"),
            "int forced_input_was_processed;\n".to_owned(),
        );
        let (output, diagnostics) = run(
            "# 9 \"generated.c\"\nint SELECTED;\n",
            &files,
            &PreprocessOptions {
                preprocessed_input: true,
                command_line_macros: vec![CommandLineMacro::Define(
                    "SELECTED=reexpanded".to_owned(),
                )],
                forced_includes: vec![PathBuf::from("/project/forced.h")],
                suppress_line_markers: true,
                ..PreprocessOptions::default()
            },
        );
        assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
        let rendered = render_preprocessed(&output, true);
        assert!(rendered.contains("int SELECTED;"), "{rendered}");
        assert!(!rendered.contains("reexpanded"), "{rendered}");
        assert!(
            !rendered.contains("forced_input_was_processed"),
            "{rendered}"
        );
    }

    #[test]
    fn rejects_invalid_macro_replacement_lists_when_defined() {
        for (source, code) in [
            ("#define BAD(x) # 1\n", "CCC1106"),
            ("#define BAD(x) __VA_ARGS__\n", "CCC1112"),
            ("#define BAD(...) __VA_OPT__(,)\n", "CCC1111"),
        ] {
            let (output, diagnostics) = run(
                source,
                &MemoryFiles::default(),
                &PreprocessOptions::default(),
            );
            assert!(output.had_errors, "{source}");
            assert!(
                diagnostics
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == code),
                "{source}: {:#?}",
                diagnostics.diagnostics
            );
        }
    }

    #[test]
    fn rejects_preprocessor_integer_constants_wider_than_uintmax() {
        let (output, diagnostics) = run(
            "#if 18446744073709551616\n1\n#endif\n",
            &MemoryFiles::default(),
            &PreprocessOptions::default(),
        );
        assert!(output.had_errors);
        assert!(
            diagnostics
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CCC1211")
        );
    }
}
