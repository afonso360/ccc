use std::path::PathBuf;

use ccc_target::{LanguageMode, TrigraphPolicy};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DumpKind {
    PpTokens,
    Tokens,
    Ast,
    Ir,
    Clif,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrimaryAction {
    Compile { link: bool },
    Preprocess,
    Dump(DumpKind),
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
    pub output: Option<PathBuf>,
    pub language_mode: LanguageMode,
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
    pub dependencies: DependencyOptions,
    pub suppress_warnings: bool,
    pub warnings_as_errors: bool,
    pub warning_options: Vec<String>,
    pub error_limit: Option<usize>,
}

pub(crate) enum ParsedCommand {
    Run(Box<DriverOptions>),
    Help,
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<ParsedCommand, String> {
    let mut arguments = arguments.into_iter();
    let mut compile_only = false;
    let mut preprocess_only = false;
    let mut dump = None;
    let mut output = None;
    let mut inputs = Vec::new();
    let mut language_mode = LanguageMode::Gnu11;
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
    let mut dependencies = DependencyOptions::default();
    let mut suppress_warnings = false;
    let mut warnings_as_errors = false;
    let mut warning_options = Vec::new();
    let mut error_limit = None;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-c" => compile_only = true,
            "-E" => preprocess_only = true,
            "-P" => suppress_linemarkers = true,
            "-dM" => dump_macros = true,
            "--dump-pp-tokens" => select_dump(&mut dump, DumpKind::PpTokens)?,
            "--dump-tokens" => select_dump(&mut dump, DumpKind::Tokens)?,
            "--dump-ast" => select_dump(&mut dump, DumpKind::Ast)?,
            "--dump-ir" => select_dump(&mut dump, DumpKind::Ir)?,
            "--emit=clif" => select_dump(&mut dump, DumpKind::Clif)?,
            "-trigraphs" => trigraphs = TrigraphPolicy::Enabled,
            "-nostdinc" => no_standard_includes = true,
            "-nobuiltininc" => no_builtin_includes = true,
            "-w" => suppress_warnings = true,
            "-Werror" => warnings_as_errors = true,
            "-Wno-error" => warnings_as_errors = false,
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
            "--" => inputs.extend(arguments.by_ref().map(PathBuf::from)),
            _ if argument == "-std=gnu11" => language_mode = LanguageMode::Gnu11,
            _ if argument == "-std=c11" => language_mode = LanguageMode::C11,
            _ if argument.starts_with("-std=") => {
                return Err(format!("ccc: unsupported language mode `{argument}`"));
            }
            _ if let Some(value) = argument.strip_prefix("--sysroot=") => {
                sysroot = Some(PathBuf::from(value));
            }
            _ if let Some(value) = argument.strip_prefix("-ferror-limit=") => {
                error_limit = Some(parse_limit(value, "-ferror-limit")?);
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
            _ if argument.starts_with("-W") => warning_options.push(argument),
            _ if argument.starts_with('-') => {
                return Err(format!("ccc: unsupported option `{argument}`"));
            }
            _ => inputs.push(PathBuf::from(argument)),
        }
    }

    if inputs.len() != 1 {
        return Err(
            "ccc: accepts exactly one C source input; use `ccc --help` for usage".to_owned(),
        );
    }
    if compile_only && preprocess_only {
        return Err("ccc: `-c` and `-E` cannot be combined".to_owned());
    }
    if dump.is_some() && (compile_only || preprocess_only || output.is_some()) {
        return Err("ccc: dump modes cannot be combined with `-c`, `-E`, or `-o`".to_owned());
    }
    if dump.is_some() && matches!(dependencies.mode, DependencyMode::Only { .. }) {
        return Err("ccc: dump modes cannot be combined with `-M` or `-MM`".to_owned());
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
    } else {
        PrimaryAction::Compile {
            link: !compile_only,
        }
    };

    if matches!(dependencies.mode, DependencyMode::Only { .. }) {
        suppress_warnings = true;
    }

    Ok(ParsedCommand::Run(Box::new(DriverOptions {
        action,
        input: inputs.pop().expect("input count was checked"),
        output,
        language_mode,
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
        dependencies,
        suppress_warnings,
        warnings_as_errors,
        warning_options,
        error_limit,
    })))
}

fn select_dump(slot: &mut Option<DumpKind>, kind: DumpKind) -> Result<(), String> {
    if slot.replace(kind).is_some() {
        return Err("ccc: only one dump or emit mode may be selected".to_owned());
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
    fn zero_error_limit_disables_the_limit() {
        assert_eq!(
            options(&["-ferror-limit=0", "input.c"]).error_limit,
            Some(0)
        );
    }
}
