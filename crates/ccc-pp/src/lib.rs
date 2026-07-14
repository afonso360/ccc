//! C preprocessing-token formation and macro preprocessing.
//!
//! The compatibility [`lex`] entry point only forms preprocessing tokens.  The
//! [`preprocess`] entry point implements directives, macro expansion, header
//! lookup, pragma events, dependency collection, and preprocessor rendering.

mod condition;
mod diagnostic;
mod engine;
mod files;
mod lexer;
mod literal;
mod macros;
mod normalize;
mod options;
mod render;
mod token;

pub use diagnostic::{
    DiagnosticSink, PpDiagnostic, PpDiagnosticCategory, PpSecondarySpan, PpSeverity,
    VecDiagnosticSink,
};
pub use engine::{
    Dependency, DependencyEdge, DependencyGraph, DiagnosticPragmaAction, LineMarker, MacroSnapshot,
    MacroSnapshotEntry, PpItem, PragmaEvent, PreprocessContext, PreprocessOutput, preprocess,
};
pub use files::{FileIdentity, FileProvider, FsFileProvider, LoadedFile};
pub use lexer::{LexError, LexerOptions, lex, lex_with_options};
pub use literal::{
    CharacterConstant, CharacterConstantPrefix, FloatingConstant, FloatingConstantSuffix,
    IntegerConstant, IntegerSuffix, LiteralError, StringLiteral, StringLiteralPrefix,
    concatenate_string_literals, decode_character_constant, decode_floating_constant,
    decode_integer_constant, decode_string_literal,
};
pub use options::{
    CommandLineMacro, DependencyMode, IncludePath, IncludePathKind, LanguageMode, PreprocessLimits,
    PreprocessOptions,
};
pub use render::{
    DependencyRenderOptions, render_dependencies, render_macro_definitions, render_preprocessed,
};
pub use token::{PpToken, PpTokenKind};
