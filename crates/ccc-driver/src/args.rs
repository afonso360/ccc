use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ccc_diag::DiagnosticFormat;
use ccc_target::{LanguageMode, OptimizationLevel, RelocationModel, TrigraphPolicy};

use crate::dependency::default_dependency_path;
use crate::warnings::validate_warning_option;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DumpKind {
    PpTokens,
    Tokens,
    Ast,
    TypedAst,
    Ir,
    Abi,
    Clif,
    CodegenStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrimaryAction {
    Compile { link: bool },
    Preprocess,
    SyntaxOnly,
    Dump(DumpKind),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LinkOutputKind {
    #[default]
    Executable,
    Shared,
    Relocatable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LinkItem {
    Input {
        path: PathBuf,
        language: Option<DriverInputLanguage>,
    },
    Argument(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DriverInputLanguage {
    C,
    PreprocessedC,
    Assembly,
    PreprocessedAssembly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MacroAction {
    Define(String),
    Undefine(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IncludePathKind {
    Quote,
    User,
    System,
    After,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IncludePathOption {
    pub kind: IncludePathKind,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForcedInputKind {
    Macros,
    Include,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ForcedInputOption {
    pub kind: ForcedInputKind,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DependencyMode {
    None,
    Only { include_system: bool },
    SideEffect { include_system: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DependencyTarget {
    Literal(String),
    Quoted(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DependencyOptions {
    pub mode: DependencyMode,
    pub output: Option<PathBuf>,
    pub targets: Vec<DependencyTarget>,
    pub phony_targets: bool,
}

impl Default for DependencyOptions {
    fn default() -> Self {
        Self {
            mode: DependencyMode::None,
            output: None,
            targets: Vec::new(),
            phony_targets: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DriverOptions {
    pub action: PrimaryAction,
    pub input: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub input_languages: Vec<Option<DriverInputLanguage>>,
    pub link_items: Vec<LinkItem>,
    pub link_output_kind: LinkOutputKind,
    pub output: Option<PathBuf>,
    pub phase_timings_output: Option<PathBuf>,
    pub language_mode: LanguageMode,
    pub relocation_model: RelocationModel,
    pub optimization: OptimizationLevel,
    pub trigraphs: TrigraphPolicy,
    pub suppress_linemarkers: bool,
    pub dump_macros: bool,
    pub macro_actions: Vec<MacroAction>,
    pub include_paths: Vec<IncludePathOption>,
    pub forced_inputs: Vec<ForcedInputOption>,
    pub no_standard_includes: bool,
    pub no_builtin_includes: bool,
    pub sysroot: Option<PathBuf>,
    pub resource_dir: Option<PathBuf>,
    pub target: Option<String>,
    pub target_arch: Option<String>,
    pub target_cpu: Option<String>,
    pub target_abi: Option<String>,
    pub sdk_root: Option<PathBuf>,
    pub deployment_target: Option<String>,
    pub dependencies: DependencyOptions,
    pub suppress_warnings: bool,
    pub warnings_as_errors: bool,
    pub warning_options: Vec<String>,
    pub error_limit: Option<usize>,
    pub diagnostic_format: DiagnosticFormat,
    pub debug_info: bool,
    pub verbose: bool,
    pub degraded_hardening: Vec<String>,
    pub print_commands_only: bool,
}

#[derive(Debug)]
pub(crate) enum ParsedCommand {
    Run(Box<DriverOptions>),
    Help,
    Version,
    VerboseVersion,
    Query(DriverQuery, Box<QueryOptions>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DriverQuery {
    DumpMachine,
    DumpVersion,
    PrintProgram(String),
    PrintFile(String),
    PrintSearchDirectories,
    PrintEffectiveConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueryOptions {
    pub target: Option<String>,
    pub target_arch: Option<String>,
    pub target_cpu: Option<String>,
    pub target_abi: Option<String>,
    pub sysroot: Option<PathBuf>,
    pub resource_dir: Option<PathBuf>,
    pub sdk_root: Option<PathBuf>,
    pub deployment_target: Option<String>,
    pub language_mode: LanguageMode,
    pub relocation_model: RelocationModel,
    pub optimization: OptimizationLevel,
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<ParsedCommand, String> {
    let mut active_response_files = HashSet::new();
    let arguments = expand_response_arguments(
        arguments.into_iter().collect(),
        0,
        &mut active_response_files,
    )?;
    let mut arguments = arguments.into_iter();
    let mut compile_only = false;
    let mut preprocess_only = false;
    let mut syntax_only = false;
    let mut dump = None;
    let mut output = None;
    let mut phase_timings_output = None;
    let mut inputs = Vec::new();
    let mut input_languages = Vec::new();
    let mut link_items = Vec::new();
    let mut link_output_kind = LinkOutputKind::Executable;
    let mut language_mode = LanguageMode::Gnu11;
    let mut relocation_model = RelocationModel::Pie;
    let mut optimization = OptimizationLevel::O0;
    let mut trigraphs = TrigraphPolicy::LanguageDefault;
    let mut suppress_linemarkers = false;
    let mut dump_macros = false;
    let mut macro_actions = Vec::new();
    let mut include_paths = Vec::new();
    let mut forced_inputs = Vec::new();
    let mut no_standard_includes = false;
    let mut no_builtin_includes = false;
    let mut sysroot = None;
    let mut resource_dir = None;
    let mut target = None;
    let mut target_arch = None;
    let mut target_cpu = None;
    let mut target_abi = None;
    let mut sdk_root = None;
    let mut deployment_target = None;
    let mut dependencies = DependencyOptions::default();
    let mut suppress_warnings = false;
    let mut warnings_as_errors = false;
    let mut warning_options = Vec::new();
    let mut error_limit = None;
    let mut diagnostic_format = DiagnosticFormat::Text;
    let mut debug_info = false;
    let mut verbose = false;
    let mut query = None;
    let mut degraded_hardening = Vec::new();
    let mut input_language = None;
    let mut print_commands_only = false;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-c" => compile_only = true,
            "-S" => {
                return Err(
                    "ccc: unsupported capability `-S`: faithful target assembly emission is not available"
                        .to_owned(),
                );
            }
            "-E" => preprocess_only = true,
            "-fsyntax-only" => syntax_only = true,
            "-P" => suppress_linemarkers = true,
            "-dM" => dump_macros = true,
            "--dump-pp-tokens" => select_dump(&mut dump, DumpKind::PpTokens)?,
            "--dump-tokens" => select_dump(&mut dump, DumpKind::Tokens)?,
            "--dump-ast" => select_dump(&mut dump, DumpKind::Ast)?,
            "--dump-typed-ast" => select_dump(&mut dump, DumpKind::TypedAst)?,
            "--dump-ir" => select_dump(&mut dump, DumpKind::Ir)?,
            "--dump-abi" => select_dump(&mut dump, DumpKind::Abi)?,
            "--emit=clif" => select_dump(&mut dump, DumpKind::Clif)?,
            "--emit=codegen-stats" => select_dump(&mut dump, DumpKind::CodegenStats)?,
            "--emit=obj" => compile_only = true,
            "--emit=asm" => {
                return Err(
                    "ccc: unsupported capability `--emit=asm`: annotated disassembly is not available"
                        .to_owned(),
                );
            }
            "-trigraphs" => trigraphs = TrigraphPolicy::Enabled,
            "-nostdinc" => no_standard_includes = true,
            "-nobuiltininc" => no_builtin_includes = true,
            "-w" => suppress_warnings = true,
            "-v" => verbose = true,
            "-###" => print_commands_only = true,
            "-Werror" => warnings_as_errors = true,
            "-Wno-error" => warnings_as_errors = false,
            "-fPIC" | "-fpic" => {
                relocation_model = RelocationModel::Pic;
            }
            "-fPIE" | "-fpie" | "-pie" | "--pie" => relocation_model = RelocationModel::Pie,
            "-fno-PIC" | "-fno-pic" | "-fno-PIE" | "-fno-pie" | "-no-pie" | "--no-pie"
            | "-nopie" => {
                relocation_model = RelocationModel::Static;
            }
            "-shared" | "-dynamiclib" => {
                link_output_kind = LinkOutputKind::Shared;
                relocation_model = RelocationModel::Pic;
            }
            "-r" => link_output_kind = LinkOutputKind::Relocatable,
            "-static" => {
                relocation_model = RelocationModel::Static;
                link_items.push(LinkItem::Argument(argument));
            }
            "-rdynamic" | "-s" | "-nostdlib" | "-nodefaultlibs" | "-nostartfiles"
            | "-static-libgcc" | "-shared-libgcc" => {
                link_items.push(LinkItem::Argument(argument));
            }
            "-pthread" => {
                macro_actions.push(MacroAction::Define("_REENTRANT=1".to_owned()));
                link_items.push(LinkItem::Argument(argument));
            }
            "-O0" => optimization = OptimizationLevel::O0,
            "-O" | "-O1" => optimization = OptimizationLevel::O1,
            "-O2" => optimization = OptimizationLevel::O2,
            "-O3" => optimization = OptimizationLevel::O3,
            "-Os" => optimization = OptimizationLevel::Size,
            "-Oz" => optimization = OptimizationLevel::SizeMin,
            // CCC does not use driver pipes between compilation phases, so
            // accepting this build-system preference has no observable effect.
            "-pipe" => {}
            // There is no type-based alias optimization in the baseline IR.
            // Disabling it is therefore behavior-compatible.
            "-fno-strict-aliasing" => {}
            "-fstack-protector"
            | "-fstack-protector-strong"
            | "-fstack-protector-all"
            | "-fstack-clash-protection"
            | "-fcf-protection"
            | "-mshstk"
            | "-fno-plt"
            | "-fno-omit-frame-pointer" => {
                degraded_hardening.push(argument);
            }
            // Disabling a hardening transform CCC does not perform is
            // behavior-compatible with the generated code.
            "-fno-stack-protector"
            | "-fno-stack-clash-protection"
            | "-fomit-frame-pointer"
            | "-fno-delete-null-pointer-checks"
            | "-fno-strict-overflow" => {}
            "-pedantic" => warning_options.push("-Wpedantic".to_owned()),
            "-pedantic-errors" => {
                warning_options.push("-Werror=pedantic".to_owned());
            }
            "-g" | "-g1" | "-g2" | "-g3" => debug_info = true,
            "-g0" => debug_info = false,
            "-M" => select_dependency_mode(
                &mut dependencies.mode,
                DependencyMode::Only {
                    include_system: true,
                },
            )?,
            "-MM" => select_dependency_mode(
                &mut dependencies.mode,
                DependencyMode::Only {
                    include_system: false,
                },
            )?,
            "-MD" => select_dependency_mode(
                &mut dependencies.mode,
                DependencyMode::SideEffect {
                    include_system: true,
                },
            )?,
            "-MMD" => select_dependency_mode(
                &mut dependencies.mode,
                DependencyMode::SideEffect {
                    include_system: false,
                },
            )?,
            "-MP" => dependencies.phony_targets = true,
            "-MG" => {
                return Err("ccc: unsupported dependency option `-MG`".to_owned());
            }
            "-o" => output = Some(take_path(&mut arguments, "-o")?),
            "-x" => {
                input_language = parse_input_language(&take_value(&mut arguments, "-x")?)?;
            }
            "-L" => {
                link_items.push(LinkItem::Argument("-L".to_owned()));
                link_items.push(LinkItem::Argument(take_value(&mut arguments, "-L")?));
            }
            "-l" => {
                link_items.push(LinkItem::Argument("-l".to_owned()));
                link_items.push(LinkItem::Argument(take_value(&mut arguments, "-l")?));
            }
            "-Xlinker" => {
                link_items.push(LinkItem::Argument("-Xlinker".to_owned()));
                link_items.push(LinkItem::Argument(take_value(&mut arguments, "-Xlinker")?));
            }
            "-R" | "-T" | "-u" | "-z" => {
                let option = argument.clone();
                link_items.push(LinkItem::Argument(argument));
                link_items.push(LinkItem::Argument(take_value(&mut arguments, &option)?));
            }
            "-install_name"
            | "-compatibility_version"
            | "-current_version"
            | "-undefined"
            | "-arch"
            | "-framework" => {
                let option = argument.clone();
                link_items.push(LinkItem::Argument(argument));
                link_items.push(LinkItem::Argument(take_value(&mut arguments, &option)?));
            }
            "-D" => macro_actions.push(MacroAction::Define(take_value(&mut arguments, "-D")?)),
            "-U" => {
                macro_actions.push(MacroAction::Undefine(take_value(&mut arguments, "-U")?));
            }
            "-I" => include_paths.push(IncludePathOption {
                kind: IncludePathKind::User,
                path: take_path(&mut arguments, "-I")?,
            }),
            "-iquote" => include_paths.push(IncludePathOption {
                kind: IncludePathKind::Quote,
                path: take_path(&mut arguments, "-iquote")?,
            }),
            "-isystem" => include_paths.push(IncludePathOption {
                kind: IncludePathKind::System,
                path: take_path(&mut arguments, "-isystem")?,
            }),
            "-idirafter" => include_paths.push(IncludePathOption {
                kind: IncludePathKind::After,
                path: take_path(&mut arguments, "-idirafter")?,
            }),
            "-include" => forced_inputs.push(ForcedInputOption {
                kind: ForcedInputKind::Include,
                path: take_path(&mut arguments, "-include")?,
            }),
            "-imacros" => forced_inputs.push(ForcedInputOption {
                kind: ForcedInputKind::Macros,
                path: take_path(&mut arguments, "-imacros")?,
            }),
            "-isysroot" => sysroot = Some(take_path(&mut arguments, "-isysroot")?),
            "-resource-dir" => {
                resource_dir = Some(take_path(&mut arguments, "-resource-dir")?);
            }
            "--target" | "-target" => {
                target = Some(take_value(&mut arguments, "--target")?);
            }
            "--sdk-root" => sdk_root = Some(take_path(&mut arguments, "--sdk-root")?),
            "-MF" => dependencies.output = Some(take_path(&mut arguments, "-MF")?),
            "-MT" => dependencies
                .targets
                .push(DependencyTarget::Literal(take_value(
                    &mut arguments,
                    "-MT",
                )?)),
            "-MQ" => dependencies
                .targets
                .push(DependencyTarget::Quoted(take_value(&mut arguments, "-MQ")?)),
            "-h" | "--help" => return Ok(ParsedCommand::Help),
            "--version" => return Ok(ParsedCommand::Version),
            "-dumpmachine" => select_query(&mut query, DriverQuery::DumpMachine)?,
            "-dumpversion" | "-dumpfullversion" => {
                select_query(&mut query, DriverQuery::DumpVersion)?;
            }
            "-print-search-dirs" => {
                select_query(&mut query, DriverQuery::PrintSearchDirectories)?;
            }
            "--print-effective-config" => {
                select_query(&mut query, DriverQuery::PrintEffectiveConfig)?;
            }
            "--" => {
                for argument in arguments.by_ref() {
                    let input = PathBuf::from(argument);
                    link_items.push(LinkItem::Input {
                        path: input.clone(),
                        language: input_language,
                    });
                    inputs.push(input);
                    input_languages.push(input_language);
                }
            }
            _ if matches!(argument.as_str(), "-std=gnu11" | "-std=gnu99") => {
                // The supported GNU C11 language is a source-compatible
                // superset for the C99 build profiles accepted here.
                language_mode = LanguageMode::Gnu11;
            }
            _ if matches!(argument.as_str(), "-std=c11" | "-std=c99") => {
                // Likewise, the strict C11 frontend accepts the C99 build
                // profile while retaining CCC's documented C11 predefined
                // macro contract.
                language_mode = LanguageMode::C11;
            }
            _ if argument.starts_with("-std=") => {
                return Err(format!("ccc: unsupported language mode `{argument}`"));
            }
            _ if let Some(value) = argument.strip_prefix("--sysroot=") => {
                sysroot = Some(PathBuf::from(value));
            }
            _ if let Some(value) = argument.strip_prefix("--write-phase-timings=") => {
                require_joined_value(value, "--write-phase-timings")?;
                if phase_timings_output.replace(PathBuf::from(value)).is_some() {
                    return Err(
                        "ccc: `--write-phase-timings` may be specified only once".to_owned()
                    );
                }
            }
            _ if let Some(value) = argument
                .strip_prefix("--target=")
                .or_else(|| argument.strip_prefix("-target=")) =>
            {
                require_joined_value(value, "--target")?;
                target = Some(value.to_owned());
            }
            _ if let Some(value) = argument.strip_prefix("-march=") => {
                require_joined_value(value, "-march")?;
                target_arch = Some(value.to_owned());
            }
            _ if let Some(value) = argument.strip_prefix("-mcpu=") => {
                require_joined_value(value, "-mcpu")?;
                target_cpu = Some(value.to_owned());
            }
            _ if let Some(value) = argument.strip_prefix("-mabi=") => {
                require_joined_value(value, "-mabi")?;
                target_abi = Some(value.to_owned());
            }
            _ if let Some(value) = argument.strip_prefix("--sdk-root=") => {
                require_joined_value(value, "--sdk-root")?;
                sdk_root = Some(PathBuf::from(value));
            }
            _ if let Some(value) = argument.strip_prefix("-mmacosx-version-min=") => {
                require_joined_value(value, "-mmacosx-version-min")?;
                deployment_target = Some(value.to_owned());
            }
            _ if let Some(value) = argument.strip_prefix("-print-prog-name=") => {
                require_joined_value(value, "-print-prog-name")?;
                select_query(&mut query, DriverQuery::PrintProgram(value.to_owned()))?;
            }
            _ if let Some(value) = argument.strip_prefix("-print-file-name=") => {
                require_joined_value(value, "-print-file-name")?;
                select_query(&mut query, DriverQuery::PrintFile(value.to_owned()))?;
            }
            _ if let Some(value) = argument.strip_prefix("-ferror-limit=") => {
                error_limit = Some(parse_limit(value, "-ferror-limit")?);
            }
            _ if let Some(value) = argument.strip_prefix("-fdiagnostics-format=") => {
                diagnostic_format = match value {
                    "text" => DiagnosticFormat::Text,
                    "json" => DiagnosticFormat::Json,
                    _ => {
                        return Err(format!(
                            "ccc: unsupported diagnostics format `{value}`; expected `text` or `json`"
                        ));
                    }
                };
            }
            _ if let Some(value) = argument.strip_prefix("-fcf-protection=") => match value {
                "none" => {}
                "full" | "branch" | "return" => degraded_hardening.push(argument),
                _ => {
                    return Err(format!(
                        "ccc: unsupported control-flow protection mode `{value}`"
                    ));
                }
            },
            _ if let Some(value) = argument.strip_prefix("-x") => {
                require_joined_value(value, "-x")?;
                input_language = parse_input_language(value)?;
            }
            _ if is_debug_level_option(&argument) => {
                return Err(format!(
                    "ccc: unsupported debug-information level `{argument}`"
                ));
            }
            _ if let Some(value) = argument.strip_prefix("-D") => {
                require_joined_value(value, "-D")?;
                macro_actions.push(MacroAction::Define(value.to_owned()));
            }
            _ if let Some(value) = argument.strip_prefix("-U") => {
                require_joined_value(value, "-U")?;
                macro_actions.push(MacroAction::Undefine(value.to_owned()));
            }
            _ if let Some(value) = argument.strip_prefix("-I") => {
                require_joined_value(value, "-I")?;
                include_paths.push(IncludePathOption {
                    kind: IncludePathKind::User,
                    path: PathBuf::from(value),
                });
            }
            _ if let Some(value) = argument.strip_prefix("-Wp,") => {
                for option in value.split(',') {
                    if let Some(definition) = option.strip_prefix("-D") {
                        require_joined_value(definition, "-Wp,-D")?;
                        macro_actions.push(MacroAction::Define(definition.to_owned()));
                    } else if let Some(name) = option.strip_prefix("-U") {
                        require_joined_value(name, "-Wp,-U")?;
                        macro_actions.push(MacroAction::Undefine(name.to_owned()));
                    } else {
                        return Err(format!(
                            "ccc: unsupported preprocessor pass-through option `{option}`"
                        ));
                    }
                }
            }
            _ if argument == "-Wa"
                || argument.starts_with("-Wa,")
                || argument.starts_with("-Wa=") =>
            {
                return Err(format!(
                    "ccc: unsupported assembler pass-through option `{argument}`"
                ));
            }
            _ if argument.starts_with("-Wl,") || argument.starts_with("-Wl=") => {
                link_items.push(LinkItem::Argument(argument));
            }
            _ if let Some(value) = argument.strip_prefix("-Xlinker=") => {
                require_joined_value(value, "-Xlinker")?;
                link_items.push(LinkItem::Argument("-Xlinker".to_owned()));
                link_items.push(LinkItem::Argument(value.to_owned()));
            }
            _ if let Some(value) = argument.strip_prefix("-L") => {
                require_joined_value(value, "-L")?;
                link_items.push(LinkItem::Argument(argument));
            }
            _ if let Some(value) = argument.strip_prefix("-l") => {
                require_joined_value(value, "-l")?;
                link_items.push(LinkItem::Argument(argument));
            }
            _ if argument.starts_with("-R")
                || argument.starts_with("-T")
                || argument.starts_with("-u")
                || argument.starts_with("-z") =>
            {
                let option = &argument[..2];
                require_joined_value(&argument[2..], option)?;
                link_items.push(LinkItem::Argument(argument));
            }
            _ if let Some(value) = argument.strip_prefix("-MF") => {
                require_joined_value(value, "-MF")?;
                dependencies.output = Some(PathBuf::from(value));
            }
            _ if let Some(value) = argument.strip_prefix("-MT") => {
                require_joined_value(value, "-MT")?;
                dependencies
                    .targets
                    .push(DependencyTarget::Literal(value.to_owned()));
            }
            _ if let Some(value) = argument.strip_prefix("-MQ") => {
                require_joined_value(value, "-MQ")?;
                dependencies
                    .targets
                    .push(DependencyTarget::Quoted(value.to_owned()));
            }
            _ if argument.starts_with("-W") => {
                validate_warning_option(&argument)?;
                warning_options.push(argument);
            }
            _ if argument.starts_with('-') => {
                return Err(format!("ccc: unsupported option `{argument}`"));
            }
            _ => {
                let input = PathBuf::from(argument);
                link_items.push(LinkItem::Input {
                    path: input.clone(),
                    language: input_language,
                });
                inputs.push(input);
                input_languages.push(input_language);
            }
        }
    }

    if let Some(query) = query {
        if phase_timings_output.is_some() {
            return Err(
                "ccc: `--write-phase-timings` cannot be combined with build-system introspection"
                    .to_owned(),
            );
        }
        if !inputs.is_empty() {
            return Err(
                "ccc: build-system introspection options do not accept input files".to_owned(),
            );
        }
        return Ok(ParsedCommand::Query(
            query,
            Box::new(QueryOptions {
                target,
                target_arch,
                target_cpu,
                target_abi,
                sysroot,
                resource_dir,
                sdk_root,
                deployment_target,
                language_mode,
                relocation_model,
                optimization,
            }),
        ));
    }
    if inputs.is_empty() && phase_timings_output.is_some() {
        return Err(
            "ccc: `--write-phase-timings` requires exactly one C or preprocessed-C input"
                .to_owned(),
        );
    }
    if inputs.is_empty() && verbose {
        return Ok(ParsedCommand::VerboseVersion);
    }
    if inputs.is_empty() {
        return Err("ccc: no input files".to_owned());
    }
    if usize::from(compile_only) + usize::from(preprocess_only) + usize::from(syntax_only) > 1 {
        return Err("ccc: `-c`, `-E`, and `-fsyntax-only` are mutually exclusive".to_owned());
    }
    if dump.is_some() && (compile_only || preprocess_only || output.is_some()) {
        return Err("ccc: dump modes cannot be combined with `-c`, `-E`, or `-o`".to_owned());
    }
    if (preprocess_only || syntax_only || dump.is_some()) && inputs.len() != 1 {
        return Err(
            "ccc: preprocessing, syntax-only, and dump modes require exactly one input".to_owned(),
        );
    }
    if compile_only && inputs.len() > 1 && output.is_some() {
        return Err("ccc: cannot use `-o` with `-c` and multiple input files".to_owned());
    }
    if !matches!(link_output_kind, LinkOutputKind::Executable) && compile_only {
        return Err("ccc: link-output options cannot be combined with `-c`".to_owned());
    }
    if link_output_kind == LinkOutputKind::Shared && relocation_model == RelocationModel::Static {
        return Err("ccc: shared output requires position-independent input code".to_owned());
    }
    if dump.is_some() && matches!(dependencies.mode, DependencyMode::Only { .. }) {
        return Err("ccc: dump modes cannot be combined with `-M` or `-MM`".to_owned());
    }
    if dump == Some(DumpKind::CodegenStats)
        && dependencies.output.as_deref() == Some(Path::new("-"))
    {
        return Err(
            "ccc: `--emit=codegen-stats` requires dependency output to use a file".to_owned(),
        );
    }
    if dump_macros && !preprocess_only {
        return Err("ccc: `-dM` requires `-E`".to_owned());
    }
    if dump_macros && matches!(dependencies.mode, DependencyMode::Only { .. }) {
        return Err("ccc: `-dM` cannot be combined with `-M` or `-MM`".to_owned());
    }
    if suppress_linemarkers && !preprocess_only {
        return Err("ccc: `-P` requires `-E`".to_owned());
    }
    if dependencies.phony_targets && matches!(dependencies.mode, DependencyMode::None) {
        return Err("ccc: `-MP` requires dependency generation".to_owned());
    }
    if (dependencies.output.is_some() || !dependencies.targets.is_empty())
        && matches!(dependencies.mode, DependencyMode::None)
    {
        return Err("ccc: dependency output options require dependency generation".to_owned());
    }

    let action = if let Some(kind) = dump {
        PrimaryAction::Dump(kind)
    } else if preprocess_only || matches!(dependencies.mode, DependencyMode::Only { .. }) {
        PrimaryAction::Preprocess
    } else if syntax_only {
        PrimaryAction::SyntaxOnly
    } else {
        PrimaryAction::Compile {
            link: !compile_only,
        }
    };

    if let Some(timings_output) = &phase_timings_output {
        validate_phase_timing_action(
            &action,
            print_commands_only,
            &inputs,
            &input_languages,
            &forced_inputs,
            output.as_deref(),
            &dependencies,
            timings_output,
        )?;
    }

    if matches!(dependencies.mode, DependencyMode::Only { .. }) {
        suppress_warnings = true;
    }

    Ok(ParsedCommand::Run(Box::new(DriverOptions {
        action,
        input: inputs[0].clone(),
        inputs,
        input_languages,
        link_items,
        link_output_kind,
        output,
        phase_timings_output,
        language_mode,
        relocation_model,
        optimization,
        trigraphs,
        suppress_linemarkers,
        dump_macros,
        macro_actions,
        include_paths,
        forced_inputs,
        no_standard_includes,
        no_builtin_includes,
        sysroot,
        resource_dir,
        target,
        target_arch,
        target_cpu,
        target_abi,
        sdk_root,
        deployment_target,
        dependencies,
        suppress_warnings,
        warnings_as_errors,
        warning_options,
        error_limit,
        diagnostic_format,
        debug_info,
        verbose,
        degraded_hardening,
        print_commands_only,
    })))
}

fn expand_response_arguments(
    arguments: Vec<String>,
    depth: usize,
    active: &mut HashSet<PathBuf>,
) -> Result<Vec<String>, String> {
    if depth > 16 {
        return Err("ccc: response-file nesting exceeds 16 levels".to_owned());
    }
    let mut expanded = Vec::new();
    for argument in arguments {
        let Some(path) = argument.strip_prefix('@').filter(|path| !path.is_empty()) else {
            expanded.push(argument);
            continue;
        };
        if let Some(literal) = path.strip_prefix('@') {
            expanded.push(format!("@{literal}"));
            continue;
        }
        let spelled = PathBuf::from(path);
        let canonical = fs::canonicalize(&spelled).map_err(|error| {
            format!(
                "ccc: cannot read response file {}: {error}",
                spelled.display()
            )
        })?;
        if !active.insert(canonical.clone()) {
            return Err(format!(
                "ccc: response file {} recursively includes itself",
                spelled.display()
            ));
        }
        let contents = fs::read_to_string(&canonical).map_err(|error| {
            format!(
                "ccc: cannot read response file {}: {error}",
                spelled.display()
            )
        })?;
        let nested = parse_response_contents(&contents, &spelled)?;
        expanded.extend(expand_response_arguments(nested, depth + 1, active)?);
        active.remove(&canonical);
    }
    Ok(expanded)
}

fn parse_response_contents(contents: &str, path: &std::path::Path) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut token_started = false;
    let mut quote = None;
    let mut escaped = false;
    for character in contents.chars() {
        if escaped {
            current.push(character);
            token_started = true;
            escaped = false;
            continue;
        }
        if character == '\\' {
            token_started = true;
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            token_started = true;
            quote = Some(character);
        } else if character.is_whitespace() {
            if token_started {
                arguments.push(std::mem::take(&mut current));
                token_started = false;
            }
        } else {
            current.push(character);
            token_started = true;
        }
    }
    if escaped {
        return Err(format!(
            "ccc: response file {} ends with an incomplete escape",
            path.display()
        ));
    }
    if quote.is_some() {
        return Err(format!(
            "ccc: response file {} has an unterminated quote",
            path.display()
        ));
    }
    if token_started {
        arguments.push(current);
    }
    Ok(arguments)
}

fn select_dump(slot: &mut Option<DumpKind>, kind: DumpKind) -> Result<(), String> {
    if slot.replace(kind).is_some() {
        return Err("ccc: only one dump or emit mode may be selected".to_owned());
    }
    Ok(())
}

fn select_query(slot: &mut Option<DriverQuery>, query: DriverQuery) -> Result<(), String> {
    if slot.replace(query).is_some() {
        return Err("ccc: only one build-system introspection option may be selected".to_owned());
    }
    Ok(())
}

fn select_dependency_mode(slot: &mut DependencyMode, mode: DependencyMode) -> Result<(), String> {
    if !matches!(slot, DependencyMode::None) {
        return Err("ccc: only one dependency generation mode may be selected".to_owned());
    }
    *slot = mode;
    Ok(())
}

fn take_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("ccc: `{option}` requires an argument"))
}

fn take_path(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<PathBuf, String> {
    take_value(arguments, option).map(PathBuf::from)
}

fn require_joined_value(value: &str, option: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("ccc: `{option}` requires an argument"))
    } else {
        Ok(())
    }
}

fn parse_limit(value: &str, option: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("ccc: `{option}` requires a non-negative integer"))
}

fn parse_input_language(value: &str) -> Result<Option<DriverInputLanguage>, String> {
    match value {
        "none" => Ok(None),
        "c" => Ok(Some(DriverInputLanguage::C)),
        "c-cpp-output" | "cpp-output" => Ok(Some(DriverInputLanguage::PreprocessedC)),
        "assembler" => Ok(Some(DriverInputLanguage::Assembly)),
        "assembler-with-cpp" => Ok(Some(DriverInputLanguage::PreprocessedAssembly)),
        _ => Err(format!("ccc: unsupported input language `{value}`")),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_phase_timing_action(
    action: &PrimaryAction,
    print_commands_only: bool,
    inputs: &[PathBuf],
    input_languages: &[Option<DriverInputLanguage>],
    forced_inputs: &[ForcedInputOption],
    output: Option<&Path>,
    dependencies: &DependencyOptions,
    timings_output: &Path,
) -> Result<(), String> {
    if print_commands_only {
        return Err("ccc: `--write-phase-timings` cannot be combined with `-###`".to_owned());
    }
    match action {
        PrimaryAction::Compile { link: true } => {
            return Err(
                "ccc: `--write-phase-timings` does not support link actions; use `-c`".to_owned(),
            );
        }
        PrimaryAction::Preprocess if matches!(dependencies.mode, DependencyMode::Only { .. }) => {
            return Err(
                "ccc: `--write-phase-timings` does not support dependency-only actions".to_owned(),
            );
        }
        PrimaryAction::Preprocess => {}
        PrimaryAction::Compile { link: false }
        | PrimaryAction::SyntaxOnly
        | PrimaryAction::Dump(_) => {}
    }
    if inputs.len() != 1 {
        return Err(
            "ccc: `--write-phase-timings` requires exactly one C or preprocessed-C input"
                .to_owned(),
        );
    }
    if !phase_timing_input_is_c(&inputs[0], input_languages[0]) {
        return Err(
            "ccc: `--write-phase-timings` supports only C and preprocessed-C inputs".to_owned(),
        );
    }
    validate_phase_timing_output_paths(
        action,
        inputs,
        forced_inputs,
        output,
        dependencies,
        timings_output,
    )
}

pub(crate) fn revalidate_phase_timing_output_paths(options: &DriverOptions) -> Result<(), String> {
    let Some(timings_output) = options.phase_timings_output.as_deref() else {
        return Ok(());
    };
    validate_phase_timing_output_paths(
        &options.action,
        &options.inputs,
        &options.forced_inputs,
        options.output.as_deref(),
        &options.dependencies,
        timings_output,
    )
}

fn validate_phase_timing_output_paths(
    action: &PrimaryAction,
    inputs: &[PathBuf],
    forced_inputs: &[ForcedInputOption],
    output: Option<&Path>,
    dependencies: &DependencyOptions,
    timings_output: &Path,
) -> Result<(), String> {
    let Some(input) = inputs.first() else {
        return Err(
            "ccc: `--write-phase-timings` requires exactly one C or preprocessed-C input"
                .to_owned(),
        );
    };
    if timings_output == Path::new("-") {
        return Err("ccc: `--write-phase-timings` requires a filesystem output path".to_owned());
    }
    for input in std::iter::once(input.as_path())
        .chain(forced_inputs.iter().map(|forced| forced.path.as_path()))
    {
        if timing_paths_conflict(timings_output, input)? {
            return Err(
                "ccc: phase-timing output must be distinct from the compilation input".to_owned(),
            );
        }
    }
    if output
        .map(|output| timing_paths_conflict(timings_output, output))
        .transpose()?
        .unwrap_or(false)
    {
        return Err(
            "ccc: phase-timing output must be distinct from other driver outputs".to_owned(),
        );
    }
    let dependency_output = match dependencies.mode {
        DependencyMode::SideEffect { .. } => dependencies
            .output
            .clone()
            .or_else(|| {
                matches!(action, PrimaryAction::Preprocess)
                    .then(|| output.map(Path::to_path_buf))
                    .flatten()
            })
            .or_else(|| Some(default_dependency_path(input, output))),
        DependencyMode::None | DependencyMode::Only { .. } => None,
    };
    if dependency_output
        .as_deref()
        .map(|output| timing_paths_conflict(timings_output, output))
        .transpose()?
        .unwrap_or(false)
    {
        return Err("ccc: phase-timing output must be distinct from dependency output".to_owned());
    }
    if matches!(action, PrimaryAction::Compile { link: false })
        && output.is_none()
        && timing_paths_conflict(
            timings_output,
            &input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.to_path_buf())
                .with_extension("o"),
        )?
    {
        return Err(
            "ccc: phase-timing output must be distinct from the compilation output".to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn timing_paths_conflict(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(comparable_timing_path(left)? == comparable_timing_path(right)?)
}

fn comparable_timing_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| {
        format!(
            "ccc: cannot resolve output path {}: {error}",
            path.display()
        )
    })?;
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return Ok(canonical);
    }
    if let (Some(parent), Some(file_name)) = (absolute.parent(), absolute.file_name())
        && let Ok(parent) = fs::canonicalize(parent)
    {
        return Ok(parent.join(file_name));
    }

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn phase_timing_input_is_c(input: &Path, language: Option<DriverInputLanguage>) -> bool {
    if let Some(language) = language {
        return matches!(
            language,
            DriverInputLanguage::C | DriverInputLanguage::PreprocessedC
        );
    }
    if input
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".so."))
    {
        return false;
    }
    !matches!(
        input.extension().and_then(|extension| extension.to_str()),
        Some("s" | "S" | "o" | "lo" | "obj" | "a" | "so" | "dylib")
    )
}

fn is_debug_level_option(argument: &str) -> bool {
    argument
        .strip_prefix("-g")
        .is_some_and(|level| !level.is_empty() && level.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(arguments: &[&str]) -> DriverOptions {
        let command = parse(arguments.iter().map(|argument| (*argument).to_owned())).unwrap();
        let ParsedCommand::Run(options) = command else {
            panic!("expected runnable options");
        };
        *options
    }

    #[test]
    fn parses_ordered_macro_and_include_options() {
        let options = options(&[
            "-DVALUE=1",
            "-U",
            "VALUE",
            "-D",
            "F(x)=x",
            "-iquote",
            "quoted",
            "-Iuser",
            "-isystem",
            "system",
            "input.c",
        ]);

        assert_eq!(
            options.macro_actions,
            [
                MacroAction::Define("VALUE=1".to_owned()),
                MacroAction::Undefine("VALUE".to_owned()),
                MacroAction::Define("F(x)=x".to_owned()),
            ]
        );
        assert_eq!(options.include_paths.len(), 3);
        assert_eq!(options.include_paths[0].kind, IncludePathKind::Quote);
        assert_eq!(options.include_paths[2].kind, IncludePathKind::System);
    }

    #[test]
    fn version_is_a_no_input_driver_action() {
        assert!(matches!(
            parse(["--version".to_owned()]),
            Ok(ParsedCommand::Version)
        ));
    }

    #[test]
    fn build_system_queries_do_not_require_an_input() {
        assert!(matches!(
            parse(["-dumpmachine".to_owned()]),
            Ok(ParsedCommand::Query(DriverQuery::DumpMachine, _))
        ));
        assert!(matches!(
            parse(["-dumpversion".to_owned()]),
            Ok(ParsedCommand::Query(DriverQuery::DumpVersion, _))
        ));
        assert!(matches!(
            parse(["-print-prog-name=ld".to_owned()]),
            Ok(ParsedCommand::Query(DriverQuery::PrintProgram(name), _)) if name == "ld"
        ));
        assert!(matches!(
            parse(["--print-effective-config".to_owned()]),
            Ok(ParsedCommand::Query(DriverQuery::PrintEffectiveConfig, _))
        ));
    }

    #[test]
    fn assembly_output_fails_with_an_exact_capability_diagnostic() {
        let error = parse(["-S".to_owned(), "input.c".to_owned()]).unwrap_err();
        assert!(error.contains("faithful target assembly emission"));
    }

    #[test]
    fn classifies_distro_hardening_flags_without_accepting_unknown_codegen_flags() {
        let options = options(&[
            "-fstack-protector-strong",
            "-fstack-clash-protection",
            "-fcf-protection=branch",
            "-fno-stack-protector",
            "input.c",
        ]);
        assert_eq!(
            options.degraded_hardening,
            [
                "-fstack-protector-strong",
                "-fstack-clash-protection",
                "-fcf-protection=branch",
            ]
        );
        assert!(parse(["-fwrapv".to_owned(), "input.c".to_owned()]).is_err());
    }

    #[test]
    fn accepts_only_semantically_understood_preprocessor_pass_throughs() {
        let options = options(&["-Wp,-DFORTIFIED=2,-UOLD", "input.c"]);
        assert_eq!(
            options.macro_actions,
            [
                MacroAction::Define("FORTIFIED=2".to_owned()),
                MacroAction::Undefine("OLD".to_owned()),
            ]
        );
        assert!(parse(["-Wp,-traditional".to_owned(), "input.c".to_owned()]).is_err());
    }

    #[test]
    fn warning_options_are_registry_checked() {
        let options = options(&[
            "-W",
            "-Wall",
            "-Wextra",
            "-Winline",
            "-Wno-missing-field-initializers",
            "-Werror=deprecated-declarations",
            "-Wstrict-prototypes",
            "input.c",
        ]);
        assert_eq!(
            options.warning_options,
            [
                "-W",
                "-Wall",
                "-Wextra",
                "-Winline",
                "-Wno-missing-field-initializers",
                "-Werror=deprecated-declarations",
                "-Wstrict-prototypes",
            ]
        );

        for option in [
            "-Wtypoed-category",
            "-Wno-typoed-category",
            "-Werror=typoed-category",
            "-Wno-error=typoed-category",
            "-Werror=",
            "-Wno-error=",
        ] {
            let error = parse([option.to_owned(), "input.c".to_owned()]).unwrap_err();
            assert!(
                error.contains("unknown warning option"),
                "{option}: {error}"
            );
        }
    }

    #[test]
    fn assembler_pass_throughs_are_not_misclassified_as_warnings() {
        for option in ["-Wa", "-Wa,-mrelax-relocations=no", "-Wa=--fatal-warnings"] {
            let error = parse([option.to_owned(), "input.c".to_owned()]).unwrap_err();
            assert!(
                error.contains("unsupported assembler pass-through option"),
                "{option}: {error}"
            );
        }
    }

    #[test]
    fn pedantic_errors_promotes_only_the_pedantic_category() {
        let options = options(&["-pedantic-errors", "input.c"]);
        assert!(!options.warnings_as_errors);
        assert_eq!(options.warning_options, ["-Werror=pedantic"]);
    }

    #[test]
    fn parses_preprocessing_and_dependency_behavior() {
        let options = options(&[
            "-E",
            "-P",
            "-MMD",
            "-MF",
            "deps.d",
            "-MT",
            "obj one.o",
            "input.c",
        ]);
        assert_eq!(options.action, PrimaryAction::Preprocess);
        assert!(options.suppress_linemarkers);
        assert_eq!(
            options.dependencies.mode,
            DependencyMode::SideEffect {
                include_system: false
            }
        );
        assert_eq!(options.dependencies.output, Some(PathBuf::from("deps.d")));
    }

    #[test]
    fn selects_the_typed_ast_dump() {
        assert_eq!(
            options(&["--dump-typed-ast", "input.c"]).action,
            PrimaryAction::Dump(DumpKind::TypedAst)
        );
    }

    #[test]
    fn selects_the_abi_dump() {
        assert_eq!(
            options(&["--dump-abi", "input.c"]).action,
            PrimaryAction::Dump(DumpKind::Abi)
        );
    }

    #[test]
    fn selects_the_codegen_stats_dump() {
        assert_eq!(
            options(&["--emit=codegen-stats", "input.c"]).action,
            PrimaryAction::Dump(DumpKind::CodegenStats)
        );
    }

    #[test]
    fn parses_phase_timing_output_for_supported_single_input_actions() {
        let compile = options(&["--write-phase-timings=compile.tsv", "-c", "input.c"]);
        assert_eq!(
            compile.phase_timings_output,
            Some(PathBuf::from("compile.tsv"))
        );

        let preprocessed = options(&[
            "--write-phase-timings=preprocessed.tsv",
            "-x",
            "c-cpp-output",
            "-c",
            "input",
        ]);
        assert_eq!(
            preprocessed.phase_timings_output,
            Some(PathBuf::from("preprocessed.tsv"))
        );

        for action in ["-E", "-fsyntax-only", "--dump-ast", "--emit=clif"] {
            assert!(
                parse(["--write-phase-timings=phase.tsv", action, "input.c",].map(str::to_owned))
                    .is_ok(),
                "{action}"
            );
        }
    }

    #[test]
    fn phase_timing_output_rejects_ambiguous_or_unsupported_actions() {
        let rejected = [
            vec!["--write-phase-timings=phase.tsv", "input.c"],
            vec!["--write-phase-timings=phase.tsv", "-###", "-c", "input.c"],
            vec!["--write-phase-timings=phase.tsv", "-v"],
            vec!["--write-phase-timings=phase.tsv", "-dumpmachine"],
            vec!["--write-phase-timings=phase.tsv", "-M", "input.c"],
            vec!["--write-phase-timings=phase.tsv", "-c", "one.c", "two.c"],
            vec!["--write-phase-timings=phase.tsv", "-c", "input.s"],
            vec![
                "--write-phase-timings=phase.tsv",
                "-xassembler-with-cpp",
                "-c",
                "input",
            ],
            vec!["--write-phase-timings=-", "-c", "input.c"],
            vec!["--write-phase-timings=input.c", "-c", "input.c"],
            vec!["--write-phase-timings=./input.c", "-c", "input.c"],
            vec!["--write-phase-timings=input.o", "-c", "input.c"],
            vec!["--write-phase-timings=./input.o", "-c", "input.c"],
            vec![
                "--write-phase-timings=artifact.o",
                "-c",
                "input.c",
                "-o",
                "./artifact.o",
            ],
            vec![
                "--write-phase-timings=deps.d",
                "-c",
                "-MD",
                "-MF",
                "deps.d",
                "input.c",
            ],
            vec![
                "--write-phase-timings=forced.h",
                "-c",
                "-include",
                "./forced.h",
                "input.c",
            ],
            vec!["--write-phase-timings=./input.d", "-c", "-MD", "input.c"],
            vec![
                "--write-phase-timings=build/artifact.d",
                "-c",
                "-MD",
                "input.c",
                "-o",
                "./build/artifact.o",
            ],
        ];
        for arguments in rejected {
            assert!(
                parse(arguments.iter().copied().map(str::to_owned)).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn help_does_not_treat_post_double_dash_names_as_timing_options() {
        assert!(matches!(
            parse(["--help", "--", "--write-phase-timings=literal-input",].map(str::to_owned)),
            Ok(ParsedCommand::Help)
        ));
    }

    #[test]
    fn phase_timing_output_requires_one_nonempty_joined_path() {
        assert!(
            parse(["--write-phase-timings=", "-c", "input.c",].map(str::to_owned))
                .unwrap_err()
                .contains("requires an argument")
        );
        assert!(
            parse(
                [
                    "--write-phase-timings=first.tsv",
                    "--write-phase-timings=second.tsv",
                    "-c",
                    "input.c",
                ]
                .map(str::to_owned)
            )
            .unwrap_err()
            .contains("only once")
        );
    }

    #[cfg(unix)]
    #[test]
    fn phase_timing_output_rejects_an_existing_symlink_input_alias() {
        use std::os::unix::fs::symlink;
        use std::sync::atomic::Ordering;

        let directory = std::env::temp_dir().join(format!(
            "ccc-phase-path-alias-{}-{}",
            std::process::id(),
            crate::TEMPORARY_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("source.c");
        let alias = directory.join("alias.c");
        fs::write(&source, "int value;\n").unwrap();
        symlink(&source, &alias).unwrap();

        let error = parse([
            format!("--write-phase-timings={}", source.display()),
            "-c".to_owned(),
            alias.display().to_string(),
        ])
        .unwrap_err();
        assert!(error.contains("distinct from the compilation input"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn post_execution_recheck_resolves_case_folded_output_aliases() {
        use std::sync::atomic::Ordering;

        let directory = std::env::temp_dir().join(format!(
            "ccc-phase-case-alias-{}-{}",
            std::process::id(),
            crate::TEMPORARY_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        let probe = directory.join("case-probe");
        fs::write(&probe, "probe").unwrap();
        if !directory.join("CASE-PROBE").exists() {
            fs::remove_dir_all(directory).unwrap();
            return;
        }

        let source = directory.join("source.c");
        let output = directory.join("Artifact.o");
        let timing_output = directory.join("artifact.o");
        fs::write(&source, "int value;\n").unwrap();
        let command = parse([
            "-c".to_owned(),
            source.display().to_string(),
            "-o".to_owned(),
            output.display().to_string(),
            format!("--write-phase-timings={}", timing_output.display()),
        ])
        .unwrap();
        let ParsedCommand::Run(options) = command else {
            panic!("expected runnable options");
        };

        fs::write(&output, "ordinary output").unwrap();
        let error = revalidate_phase_timing_output_paths(&options).unwrap_err();
        assert!(error.contains("distinct from other driver outputs"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "ordinary output");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dependency_only_modes_suppress_warnings() {
        let full = options(&["-M", "input.c"]);
        assert_eq!(full.action, PrimaryAction::Preprocess);
        assert!(full.suppress_warnings);

        let user_only = options(&["-MM", "input.c"]);
        assert!(user_only.suppress_warnings);
    }

    #[test]
    fn rejects_unsupported_or_conflicting_modes() {
        assert!(parse(["-std=c17".to_owned(), "input.c".to_owned()]).is_err());
        assert!(
            parse(["-mlong-double-64".to_owned(), "input.c".to_owned()]).is_err(),
            "an ABI-changing long-double mode must fail before translation"
        );
        assert!(parse(["-M".to_owned(), "-MD".to_owned(), "input.c".to_owned()]).is_err());
        assert!(parse(["-MG".to_owned(), "input.c".to_owned()]).is_err());
        assert!(parse(["-dM".to_owned(), "-M".to_owned(), "input.c".to_owned()]).is_err());
        assert!(
            parse([
                "--dump-tokens".to_owned(),
                "-MM".to_owned(),
                "input.c".to_owned()
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_joined_dependency_output_and_targets() {
        let options = options(&[
            "-MD",
            "-MFdeps.d",
            "-MTliteral target",
            "-MQquoted target",
            "input.c",
        ]);
        assert_eq!(options.dependencies.output, Some(PathBuf::from("deps.d")));
        assert_eq!(
            options.dependencies.targets,
            [
                DependencyTarget::Literal("literal target".to_owned()),
                DependencyTarget::Quoted("quoted target".to_owned()),
            ]
        );
    }

    #[test]
    fn parses_target_toolchain_and_darwin_configuration() {
        let options = options(&[
            "--target=aarch64-apple-darwin",
            "-march=armv8-a",
            "-mcpu=generic",
            "-mabi=darwin",
            "--sdk-root=/SDK",
            "-mmacosx-version-min=14.2",
            "input.c",
        ]);
        assert_eq!(options.target.as_deref(), Some("aarch64-apple-darwin"));
        assert_eq!(options.target_arch.as_deref(), Some("armv8-a"));
        assert_eq!(options.target_cpu.as_deref(), Some("generic"));
        assert_eq!(options.target_abi.as_deref(), Some("darwin"));
        assert_eq!(options.sdk_root, Some(PathBuf::from("/SDK")));
        assert_eq!(options.deployment_target.as_deref(), Some("14.2"));
    }

    #[test]
    fn zero_error_limit_disables_the_limit() {
        assert_eq!(
            options(&["-ferror-limit=0", "input.c"]).error_limit,
            Some(0)
        );
    }

    #[test]
    fn parses_diagnostic_output_formats() {
        assert_eq!(
            options(&["-fdiagnostics-format=json", "input.c"]).diagnostic_format,
            DiagnosticFormat::Json
        );
        assert_eq!(
            options(&["-fdiagnostics-format=text", "input.c"]).diagnostic_format,
            DiagnosticFormat::Text
        );
        assert!(
            parse([
                "-fdiagnostics-format=sarif".to_owned(),
                "input.c".to_owned()
            ])
            .unwrap_err()
            .contains("expected `text` or `json`")
        );
    }

    #[test]
    fn accepts_allowlisted_debug_and_optimization_options() {
        for argument in ["-g", "-g0", "-g1", "-g2", "-g3"] {
            let options = options(&[argument, "input.c"]);
            assert_eq!(options.input, PathBuf::from("input.c"), "{argument}");
        }
        for (argument, expected) in [
            ("-O", OptimizationLevel::O1),
            ("-O0", OptimizationLevel::O0),
            ("-O1", OptimizationLevel::O1),
            ("-O2", OptimizationLevel::O2),
            ("-O3", OptimizationLevel::O3),
            ("-Os", OptimizationLevel::Size),
            ("-Oz", OptimizationLevel::SizeMin),
        ] {
            assert_eq!(
                options(&[argument, "input.c"]).optimization,
                expected,
                "{argument}"
            );
        }
        assert_eq!(
            options(&["-O3", "-O0", "-Os", "input.c"]).optimization,
            OptimizationLevel::Size
        );
        assert!(!options(&["-g", "-g0", "input.c"]).debug_info);
        assert!(options(&["-g0", "-g3", "input.c"]).debug_info);
    }

    #[test]
    fn accepts_c99_build_profile_spellings_as_supported_frontend_profiles() {
        assert_eq!(
            options(&["-std=gnu99", "input.c"]).language_mode,
            LanguageMode::Gnu11
        );
        assert_eq!(
            options(&["-std=c99", "input.c"]).language_mode,
            LanguageMode::C11
        );
    }

    #[test]
    fn preserves_link_input_and_library_order() {
        let options = options(&[
            "first.c",
            "libone.a",
            "-Lsearch",
            "-lone",
            "second.o",
            "-Wl,--as-needed",
            "-Xlinker",
            "--gc-sections",
        ]);

        assert_eq!(
            options.link_items,
            [
                LinkItem::Input {
                    path: PathBuf::from("first.c"),
                    language: None,
                },
                LinkItem::Input {
                    path: PathBuf::from("libone.a"),
                    language: None,
                },
                LinkItem::Argument("-Lsearch".to_owned()),
                LinkItem::Argument("-lone".to_owned()),
                LinkItem::Input {
                    path: PathBuf::from("second.o"),
                    language: None,
                },
                LinkItem::Argument("-Wl,--as-needed".to_owned()),
                LinkItem::Argument("-Xlinker".to_owned()),
                LinkItem::Argument("--gc-sections".to_owned()),
            ]
        );
    }

    #[test]
    fn input_language_selection_is_ordered_and_resettable() {
        let options = options(&[
            "-x",
            "c",
            "extensionless",
            "-xassembler",
            "startup",
            "-xnone",
            "ordinary.S",
        ]);
        assert_eq!(
            options.input_languages,
            [
                Some(DriverInputLanguage::C),
                Some(DriverInputLanguage::Assembly),
                None,
            ]
        );
    }

    #[test]
    fn distinguishes_preprocessed_c_from_c_source() {
        let options = options(&["-x", "c-cpp-output", "generated", "-x", "c", "ordinary"]);
        assert_eq!(
            options.input_languages,
            [
                Some(DriverInputLanguage::PreprocessedC),
                Some(DriverInputLanguage::C),
            ]
        );
    }

    #[test]
    fn parses_shared_and_relocatable_link_modes() {
        let shared = options(&["-shared", "input.c"]);
        assert_eq!(shared.link_output_kind, LinkOutputKind::Shared);
        assert_eq!(shared.relocation_model, RelocationModel::Pic);

        let relocatable = options(&["-r", "input.o"]);
        assert_eq!(relocatable.link_output_kind, LinkOutputKind::Relocatable);
    }

    #[test]
    fn expands_nested_response_files_with_quotes_and_empty_arguments() {
        let directory = std::env::temp_dir().join(format!(
            "ccc-driver-response-{}-{}",
            std::process::id(),
            crate::TEMPORARY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let nested = directory.join("nested.rsp");
        let outer = directory.join("outer.rsp");
        fs::write(&nested, "'second input.o' -lanswer").unwrap();
        fs::write(
            &outer,
            format!("input.c @{} -D'EMPTY=' \"\"", nested.display()),
        )
        .unwrap();

        let expanded = expand_response_arguments(
            vec![format!("@{}", outer.display())],
            0,
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(
            expanded,
            ["input.c", "second input.o", "-lanswer", "-DEMPTY=", ""]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn validates_multiple_input_output_combinations() {
        assert!(parse(["-c", "one.c", "two.c", "-o", "both.o"].map(str::to_owned)).is_err());
        assert!(
            parse(["-E", "one.c", "two.c"].map(str::to_owned)).is_err(),
            "preprocessing remains a single-input action"
        );
        assert!(
            parse(["-shared", "-no-pie", "one.c"].map(str::to_owned)).is_err(),
            "shared output cannot select the static relocation model"
        );
    }

    #[test]
    fn relocation_options_select_a_coupled_codegen_and_link_model() {
        assert_eq!(options(&["input.c"]).relocation_model, RelocationModel::Pie);
        for argument in ["-fPIC", "-fpic"] {
            assert_eq!(
                options(&["-no-pie", argument, "input.c"]).relocation_model,
                RelocationModel::Pic,
                "{argument}"
            );
        }
        for argument in ["-fPIE", "-fpie", "-pie"] {
            assert_eq!(
                options(&["-no-pie", argument, "input.c"]).relocation_model,
                RelocationModel::Pie,
                "{argument}"
            );
        }
        for argument in ["-fno-PIC", "-fno-pic", "-fno-PIE", "-fno-pie", "-no-pie"] {
            assert_eq!(
                options(&["-pie", argument, "input.c"]).relocation_model,
                RelocationModel::Static,
                "{argument}"
            );
        }
    }

    #[test]
    fn rejects_unlisted_debug_and_optimization_options() {
        for argument in [
            "-ggdb",
            "-gline-tables-only",
            "-g-1",
            "-g17",
            "-Og",
            "-O4",
            "-Ofast",
        ] {
            assert!(
                parse([argument.to_owned(), "input.c".to_owned()]).is_err(),
                "{argument} must remain an unsupported option"
            );
        }
    }
}
