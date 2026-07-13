use std::collections::BTreeMap;
use std::path::PathBuf;

pub use ccc_target::LanguageMode;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IncludePathKind {
    Quote,
    User,
    System,
    Resource,
    After,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludePath {
    pub path: PathBuf,
    pub kind: IncludePathKind,
}

impl IncludePath {
    pub fn new(path: impl Into<PathBuf>, kind: IncludePathKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandLineMacro {
    Define(String),
    Undefine(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DependencyMode {
    #[default]
    None,
    All,
    User,
    SideEffectAll,
    SideEffectUser,
}

impl DependencyMode {
    pub const fn excludes_system_headers(self) -> bool {
        matches!(self, Self::User | Self::SideEffectUser)
    }

    pub const fn is_dependency_only(self) -> bool {
        matches!(self, Self::All | Self::User)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreprocessLimits {
    pub include_depth: usize,
    pub expansion_depth: usize,
    pub argument_depth: usize,
    pub output_tokens: usize,
    pub diagnostics: usize,
}

impl Default for PreprocessLimits {
    fn default() -> Self {
        Self {
            include_depth: 200,
            expansion_depth: 256,
            argument_depth: 256,
            output_tokens: 2_000_000,
            diagnostics: 100,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreprocessOptions {
    pub language_mode: LanguageMode,
    /// Overrides the language mode when present.
    pub trigraphs: Option<bool>,
    pub warn_trigraphs: bool,
    pub include_paths: Vec<IncludePath>,
    pub command_line_macros: Vec<CommandLineMacro>,
    pub imacros: Vec<PathBuf>,
    pub forced_includes: Vec<PathBuf>,
    pub predefined_macros: BTreeMap<String, String>,
    pub features: BTreeMap<String, bool>,
    pub dependency_mode: DependencyMode,
    pub suppress_line_markers: bool,
    pub preserve_comments: bool,
    pub gnu_comma_elision: bool,
    pub limits: PreprocessLimits,
    /// Reproducible spellings, including surrounding quotes.
    pub date_macro: String,
    pub time_macro: String,
}

impl Default for PreprocessOptions {
    fn default() -> Self {
        Self {
            language_mode: LanguageMode::Gnu11,
            trigraphs: None,
            warn_trigraphs: true,
            include_paths: Vec::new(),
            command_line_macros: Vec::new(),
            imacros: Vec::new(),
            forced_includes: Vec::new(),
            predefined_macros: BTreeMap::new(),
            features: BTreeMap::new(),
            dependency_mode: DependencyMode::None,
            suppress_line_markers: false,
            preserve_comments: false,
            gnu_comma_elision: true,
            limits: PreprocessLimits::default(),
            date_macro: "\"Jan  1 1970\"".to_owned(),
            time_macro: "\"00:00:00\"".to_owned(),
        }
    }
}

impl PreprocessOptions {
    pub const fn trigraphs_enabled(&self) -> bool {
        match self.trigraphs {
            Some(enabled) => enabled,
            None => matches!(self.language_mode, LanguageMode::C11),
        }
    }
}
