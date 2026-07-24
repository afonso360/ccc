//! The `ccc` command-line driver.

mod args;
mod atomic_output;
mod dependency;
mod diagnostics;
mod empty_object;
mod predefined;
mod resource;
mod warnings;

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use args::{
    DependencyMode as DriverDependencyMode, DependencyTarget, DriverInputLanguage, DriverOptions,
    DriverQuery, DumpKind, ForcedInputKind, IncludePathKind as DriverIncludePathKind, LinkItem,
    LinkOutputKind as DriverLinkOutputKind, MacroAction, ParsedCommand, PrimaryAction,
    QueryOptions,
};
use ccc_codegen::{Options as CodegenOptions, Output as CodegenOutput};
use ccc_diag::{
    Diagnostic, DiagnosticEngine, DiagnosticFormat, EmitOutcome, RenderOptions,
    render_json_document,
};
use ccc_ir::generic::{FullModule, IrError as FrontendIrError};
use ccc_link::{RegisteredTemporaryFile, ToolchainRequirements, ToolchainResolver};
use ccc_pp::{
    CommandLineMacro, DependencyMode, FileProvider, FsFileProvider, IncludePath, IncludePathKind,
    PpItem, PpToken, PreprocessContext, PreprocessLimits, PreprocessOptions, PreprocessOutput,
    preprocess, render_macro_definitions, render_preprocessed,
};
use ccc_sema::generic::{
    FullTypedTranslationUnit, analyze_frontend_with_recovery_limit, dump_frontend_typed_ast,
};
use ccc_session::{Session, SourceFileSpec, SourceMap};
use ccc_syntax::frontend::{
    FrontendItem, PoisonedBinding, TokenKind as FrontendTokenKind,
    TranslationUnit as FrontendTranslationUnit, convert_pp_items, dump_ast as dump_frontend_ast,
    parse_recovering_with_mode_and_limit as parse_frontend_recovering_with_mode_and_limit,
};
use ccc_target::{CompatibilityScope, EffectiveCompilationConfig, SystemIncludeKind};

use dependency::{
    DependencyRecord, DependencyRenderOptions, MakeTarget, default_dependency_path,
    render_dependencies,
};
use diagnostics::PreprocessorDiagnostics;
use resource::ResourceDirectory;
use warnings::{WarningDisposition, WarningPolicy};

pub use empty_object::is_empty_elf64_relocatable;

const HELP: &str = "Usage: ccc [options] <input>...\n\
  -c                         Compile without linking\n\
  -E [-P]                    Preprocess only; -P suppresses linemarkers\n\
  -fsyntax-only              Parse and analyze without emitting an object\n\
  -fdiagnostics-format=kind  Render diagnostics as text or versioned JSON\n\
  -x language                Select c, c-cpp-output, assembler, assembler-with-cpp, or none\n\
  -O|-O0|-O1|-O2|-O3|-Os|-Oz Select the optimization profile\n\
  -g|-g1|-g2|-g3            Emit source-level DWARF; -g0 disables it\n\
  -fPIC|-fPIE|-pie           Select position-independent code and PIE linking (default)\n\
  -fno-pic|-fno-pie|-no-pie Select static-model code and non-PIE linking\n\
  -shared|-dynamiclib        Link a shared library\n\
  -r                         Produce a relocatable linked object\n\
  -L dir -lname             Add an ordered library search path or library\n\
  -Wl,arg -Xlinker arg       Pass an ordered option to the target linker driver\n\
  -pthread                   Select the threaded compile and link profile\n\
  -Dname[=value] -Uname      Define or undefine a macro\n\
  -I dir -iquote dir         Add user include search paths\n\
  -isystem dir -idirafter dir Add system include search paths\n\
  -include file -imacros file Process a forced input\n\
  -M|-MM|-MD|-MMD            Generate Make dependencies\n\
  --target triple            Select an enabled target (defaults to the native host)\n\
  -march=name -mcpu=name     Select an enabled architecture and CPU baseline\n\
  -mabi=name                 Select an enabled ABI spelling\n\
  --sysroot dir              Select a target sysroot\n\
  --sdk-root dir             Select a Darwin SDK root\n\
  -mmacosx-version-min=ver   Select the minimum Darwin deployment version\n\
  --dump-pp-tokens           Dump expanded preprocessing tokens\n\
  --dump-tokens              Dump converted parser tokens\n\
  --dump-ast|--dump-typed-ast|--dump-ir|--dump-abi\n\
                             Dump frontend representations\n\
  --emit=clif                Dump Cranelift IR\n\
  --emit=codegen-stats       Dump stable machine-readable codegen statistics\n\
  -###                       Print replayable phase commands without executing\n\
  -dumpmachine|-dumpversion  Print build-system compiler identity\n\
  -print-prog-name=name|-print-file-name=name|-print-search-dirs\n\
                             Query the resolved target toolchain\n\
  --print-effective-config   Print normalized target, language, and toolchain facts\n\
  @file                      Read nested command arguments from a response file\n\
  --version                  Print the CCC driver version\n";

const VERSION: &str = concat!("ccc ", env!("CARGO_PKG_VERSION"), "\n");

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Eq, PartialEq)]
pub struct DriverError {
    message: String,
    json_document: Option<String>,
}

impl DriverError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            json_document: None,
        }
    }

    fn with_json_document(message: impl Into<String>, json_document: String) -> Self {
        Self {
            message: message.into(),
            json_document: Some(json_document),
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for DriverError {}

#[derive(Debug, Eq, PartialEq)]
pub struct DriverOutput {
    pub stdout: String,
    pub stderr: String,
}

pub fn run(arguments: impl IntoIterator<Item = String>) -> Result<DriverOutput, DriverError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let diagnostic_format = diagnostic_format_hint(&arguments);
    match args::parse(arguments)
        .map_err(DriverError::new)
        .map_err(|error| error_for_format(error, diagnostic_format))?
    {
        ParsedCommand::Help => Ok(DriverOutput {
            stdout: HELP.to_owned(),
            stderr: String::new(),
        }),
        ParsedCommand::Version => Ok(DriverOutput {
            stdout: VERSION.to_owned(),
            stderr: String::new(),
        }),
        ParsedCommand::VerboseVersion => Ok(DriverOutput {
            stdout: String::new(),
            stderr: verbose_version(),
        }),
        ParsedCommand::Query(query, options) => query_driver(query, *options),
        ParsedCommand::Run(options) => {
            let verbose = options.verbose;
            let hardening_diagnostics = degraded_hardening_diagnostics(&options)?;
            let format = options.diagnostic_format;
            let hardening_json = (format == DiagnosticFormat::Json
                && !hardening_diagnostics.is_empty())
            .then(|| degraded_hardening_json_document(&options, false));
            let mut output = match execute(*options) {
                Ok(output) => output,
                Err(error) => {
                    let mut error = error_for_format(error, format);
                    if let Some(hardening_json) = hardening_json {
                        error = prepend_json_to_error(&hardening_json, error);
                    }
                    return Err(error);
                }
            };
            if let Some(hardening_json) = hardening_json {
                output.stderr = if output.stderr.trim().is_empty() {
                    hardening_json
                } else {
                    merge_json_documents(&hardening_json, &output.stderr)
                        .unwrap_or_else(|| format!("{hardening_json}{}", output.stderr))
                };
            } else {
                output.stderr.insert_str(0, &hardening_diagnostics);
            }
            if verbose {
                output.stderr.insert_str(0, &verbose_version());
            }
            Ok(output)
        }
    }
}

fn diagnostic_format_hint(arguments: &[String]) -> DiagnosticFormat {
    arguments
        .iter()
        .filter_map(|argument| match argument.as_str() {
            "-fdiagnostics-format=text" => Some(DiagnosticFormat::Text),
            "-fdiagnostics-format=json" => Some(DiagnosticFormat::Json),
            _ => None,
        })
        .next_back()
        .unwrap_or(DiagnosticFormat::Text)
}

fn degraded_hardening_diagnostics(options: &DriverOptions) -> Result<String, DriverError> {
    if options.degraded_hardening.is_empty() {
        return Ok(String::new());
    }
    let policy = WarningPolicy::new(
        options.suppress_warnings,
        options.warnings_as_errors,
        &options.warning_options,
    );
    let disposition = policy.disposition("degraded-hardening");
    let severity = match disposition {
        WarningDisposition::Suppressed => return Ok(String::new()),
        WarningDisposition::Warning => "warning",
        WarningDisposition::Error => "error",
    };
    let diagnostics = options
        .degraded_hardening
        .iter()
        .map(|option| {
            format!(
                "ccc: {severity}: hardening option `{option}` is accepted without its code-generation transform [-Wdegraded-hardening]\n"
            )
        })
        .collect::<String>();
    if disposition == WarningDisposition::Error {
        let error = DriverError::new(diagnostics);
        if options.diagnostic_format == DiagnosticFormat::Json {
            let json = degraded_hardening_json_document(options, true);
            Err(DriverError::with_json_document(
                json.trim_end().to_owned(),
                json,
            ))
        } else {
            Err(error)
        }
    } else {
        Ok(diagnostics)
    }
}

fn degraded_hardening_json_document(options: &DriverOptions, as_error: bool) -> String {
    let diagnostics = options
        .degraded_hardening
        .iter()
        .map(|option| {
            let mut diagnostic = Diagnostic::warning(
                "CCC6009",
                "degraded-hardening",
                format!(
                    "hardening option `{option}` is accepted without its code-generation transform"
                ),
            );
            if as_error {
                diagnostic.severity = ccc_diag::Severity::Error;
            }
            diagnostic
        })
        .collect::<Vec<_>>();
    render_json_document(&diagnostics, &SourceMap::new(), RenderOptions::default())
}

fn query_driver(query: DriverQuery, options: QueryOptions) -> Result<DriverOutput, DriverError> {
    let config = query_config(&options)?;
    let stdout = match query {
        DriverQuery::DumpMachine => format!("{}\n", config.target.triple),
        DriverQuery::DumpVersion => "4.2.1\n".to_owned(),
        DriverQuery::PrintFile(name) if name == "include" => {
            let resources = ResourceDirectory::discover(options.resource_dir.as_deref())
                .map_err(|message| owner_error("CCC6003", message))?;
            format!("{}\n", resources.root().join("include").display())
        }
        DriverQuery::PrintProgram(name) => {
            let requirements = match name.as_str() {
                "as" => ToolchainRequirements {
                    assembler: true,
                    ..ToolchainRequirements::default()
                },
                "ar" | "ranlib" => ToolchainRequirements::archive(),
                _ => ToolchainRequirements::default(),
            };
            let toolchain = resolve_query_toolchain(&config, &options, requirements)?;
            let selected = match name.as_str() {
                "as" => toolchain.assembler.as_ref(),
                "ar" => toolchain.archiver.as_ref(),
                "ranlib" => toolchain.ranlib.as_ref(),
                _ => None,
            };
            if let Some(selected) = selected {
                format!("{}\n", selected.display())
            } else {
                run_toolchain_query(
                    toolchain.compiler_driver.as_ref(),
                    &format!("-print-prog-name={name}"),
                )?
            }
        }
        DriverQuery::PrintFile(name) => {
            let toolchain =
                resolve_query_toolchain(&config, &options, ToolchainRequirements::default())?;
            run_toolchain_query(
                toolchain.compiler_driver.as_ref(),
                &format!("-print-file-name={name}"),
            )?
        }
        DriverQuery::PrintSearchDirectories => {
            let toolchain =
                resolve_query_toolchain(&config, &options, ToolchainRequirements::default())?;
            run_toolchain_query(toolchain.compiler_driver.as_ref(), "-print-search-dirs")?
        }
        DriverQuery::PrintEffectiveConfig => {
            let resources = ResourceDirectory::discover(options.resource_dir.as_deref())
                .map_err(|message| owner_error("CCC6003", message))?;
            let toolchain = resolve_query_toolchain(
                &config,
                &options,
                ToolchainRequirements::preprocess(true),
            )?;
            let relocation = match config.relocation_model {
                ccc_target::RelocationModel::Static => "static",
                ccc_target::RelocationModel::Pic => "pic",
                ccc_target::RelocationModel::Pie => "pie",
            };
            let mut output = format!(
                "target={}\narchitecture={}\ncpu={}\nabi={}\nlanguage={}\ngnu-profile={}\nrelocation={}\noptimization={}\nresource-dir={}\n",
                config.target.triple,
                config.normalized_target_arch(),
                config.normalized_target_cpu(),
                config.normalized_target_abi(),
                config.language.mode.name(),
                config.gnu_profile.as_ref().map_or_else(
                    || "none".to_owned(),
                    |profile| {
                        format!(
                            "{}.{}.{}",
                            profile.version.major, profile.version.minor, profile.version.patch
                        )
                    },
                ),
                relocation,
                config.optimization.flag(),
                resources.root().display(),
            );
            output.push_str(&format!(
                "compiler-driver={}\nsysroot={}\n",
                toolchain
                    .compiler_driver
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), ccc_target::ToolCommandSpec::display),
                toolchain
                    .sysroot
                    .as_deref()
                    .map_or_else(|| "none".to_owned(), |path| path.display().to_string()),
            ));
            for include in &toolchain.system_includes {
                output.push_str(&format!(
                    "system-include={:?}:{}\n",
                    include.kind,
                    include.path.display()
                ));
            }
            output
        }
    };
    Ok(DriverOutput {
        stdout,
        stderr: String::new(),
    })
}

fn query_config(options: &QueryOptions) -> Result<EffectiveCompilationConfig, DriverError> {
    let mut config = (if let Some(target) = &options.target {
        let triple = target.parse().map_err(|error| {
            owner_error(
                "CCC6005",
                format!("invalid target triple `{target}`: {error}"),
            )
        })?;
        EffectiveCompilationConfig::for_target(triple)
            .map_err(|message| owner_error("CCC6005", message))?
    } else {
        EffectiveCompilationConfig::host().map_err(|message| owner_error("CCC6005", message))?
    })
    .with_language_mode(options.language_mode);
    if let Some(architecture) = &options.target_arch {
        config = config.with_target_arch(architecture);
    }
    if let Some(cpu) = &options.target_cpu {
        config = config.with_target_cpu(cpu);
    }
    if let Some(abi) = &options.target_abi {
        config = config.with_target_abi(abi);
    }
    if let Some(sdk_root) = &options.sdk_root {
        config = config.with_sdk_root(sdk_root);
    }
    if let Some(version) = &options.deployment_target {
        config = config.with_deployment_target(version);
    }
    config.relocation_model = options.relocation_model;
    config.optimization = options.optimization;
    if options.sdk_root.is_some() && config.target.abi != ccc_target::AbiIdentity::DarwinArm64 {
        return Err(owner_error(
            "CCC6005",
            "`--sdk-root` is valid only for the Darwin arm64 target",
        ));
    }
    config
        .validate_target_profile_options()
        .map_err(|message| owner_error("CCC6005", message))?;
    Ok(config)
}

fn resolve_query_toolchain(
    config: &EffectiveCompilationConfig,
    options: &QueryOptions,
    requirements: ToolchainRequirements,
) -> Result<ccc_target::ToolchainSpec, DriverError> {
    let mut resolver = ToolchainResolver::new(config);
    if let Some(sysroot) = options.sysroot.as_ref().or(options.sdk_root.as_ref()) {
        resolver = resolver.sysroot(sysroot);
    }
    resolver
        .resolve(requirements)
        .map_err(|error| owner_error(error.code, error.message))
}

fn run_toolchain_query(
    driver: Option<&ccc_target::ToolCommandSpec>,
    argument: &str,
) -> Result<String, DriverError> {
    let driver = driver.ok_or_else(|| owner_error("CCC6005", "no target compiler driver"))?;
    let output = Command::new(&driver.program)
        .args(&driver.arguments)
        .arg(argument)
        .output()
        .map_err(|error| {
            owner_error(
                "CCC6005",
                format!(
                    "cannot run target compiler driver `{}`: {error}",
                    driver.display()
                ),
            )
        })?;
    if !output.status.success() {
        return Err(owner_error(
            "CCC6005",
            format!(
                "target compiler driver `{}` rejected `{argument}`: {}",
                driver.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.ends_with('\n') {
        stdout.push('\n');
    }
    Ok(stdout)
}

fn verbose_version() -> String {
    let target = ccc_target::EffectiveCompilationConfig::host()
        .map(|config| config.target.triple.to_string())
        .unwrap_or_else(|_| "unknown-target".to_owned());
    format!(
        "ccc {} (gcc-compatible profile 4.2.1)\nTarget: {target}\n",
        env!("CARGO_PKG_VERSION")
    )
}

fn execute(options: DriverOptions) -> Result<DriverOutput, DriverError> {
    if options.print_commands_only {
        return print_command_plan(&options);
    }
    if matches!(options.action, PrimaryAction::Compile { link: true }) {
        return link_output(options);
    }
    if matches!(options.action, PrimaryAction::Compile { link: false }) && options.inputs.len() > 1
    {
        return compile_multiple_outputs(options);
    }
    if matches!(options.action, PrimaryAction::Compile { link: false })
        && matches!(
            classify_input(
                &options.input,
                options.input_languages.first().copied().flatten()
            ),
            InputKind::Assembly | InputKind::PreprocessedAssembly
        )
    {
        let (config, _) = effective_config(&options)?;
        let output = options
            .output
            .clone()
            .unwrap_or_else(|| compile_output_path(&options.input));
        return assemble_input(
            &options,
            &options.input,
            options.input_languages.first().copied().flatten(),
            &output,
            &config,
        );
    }
    let prepared = preprocess_source(&options)?;

    if matches!(options.dependencies.mode, DriverDependencyMode::Only { .. }) {
        return dependency_only_output(&options, prepared);
    }

    match options.action {
        PrimaryAction::Preprocess => preprocess_output(&options, prepared),
        PrimaryAction::SyntaxOnly => syntax_only_output(prepared),
        PrimaryAction::Dump(kind) => dump_output(&options, prepared, kind),
        PrimaryAction::Compile { link } => compile_output(&options, prepared, link),
    }
}

fn print_command_plan(options: &DriverOptions) -> Result<DriverOutput, DriverError> {
    if !matches!(options.action, PrimaryAction::Compile { .. }) {
        return Err(DriverError::new(
            "ccc: `-###` currently requires a compile or link action",
        ));
    }
    // Resolve the same target/toolchain contract as a real invocation, but do
    // not create outputs or start compilation phases.
    let (config, _) = effective_config(options)?;
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ccc"));
    let mut lines = Vec::new();

    if matches!(options.action, PrimaryAction::Compile { link: false }) {
        for (index, input) in options.inputs.iter().enumerate() {
            let output = options
                .output
                .clone()
                .unwrap_or_else(|| compile_output_path(input));
            lines.push(render_ccc_compile_command(
                &executable,
                options,
                input,
                options.input_languages[index],
                &output,
            ));
        }
    } else {
        let output = options
            .output
            .clone()
            .unwrap_or_else(|| match options.link_output_kind {
                DriverLinkOutputKind::Executable => PathBuf::from("a.out"),
                DriverLinkOutputKind::Shared => PathBuf::from("a.so"),
                DriverLinkOutputKind::Relocatable => PathBuf::from("a.o"),
            });
        let plan_directory = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut linked = Vec::<OsString>::new();
        let mut compiled_index = 0usize;
        for item in &options.link_items {
            match item {
                LinkItem::Argument(argument) => linked.push(argument.into()),
                LinkItem::Input { path, language } => {
                    if classify_input(path, *language) == InputKind::Linker {
                        linked.push(path.as_os_str().to_owned());
                    } else {
                        let temporary =
                            plan_directory.join(format!(".ccc-command-plan-{compiled_index}.o"));
                        compiled_index += 1;
                        lines.push(render_ccc_compile_command(
                            &executable,
                            options,
                            path,
                            *language,
                            &temporary,
                        ));
                        linked.push(temporary.into_os_string());
                    }
                }
            }
        }
        let mut command = vec![executable.as_os_str().to_owned()];
        append_target_command_arguments(&mut command, options);
        match options.link_output_kind {
            DriverLinkOutputKind::Executable => command.push(
                match options.relocation_model {
                    ccc_target::RelocationModel::Static => "-no-pie",
                    ccc_target::RelocationModel::Pic => "-fPIC",
                    ccc_target::RelocationModel::Pie => "-pie",
                }
                .into(),
            ),
            DriverLinkOutputKind::Shared => command.push("-shared".into()),
            DriverLinkOutputKind::Relocatable => command.push("-r".into()),
        }
        command.extend(linked);
        command.push("-o".into());
        command.push(output.as_os_str().to_owned());
        lines.push(render_shell_command(&command));
        if should_materialize_darwin_debug_artifact(options, &config) {
            let tool = resolve_dsymutil(&config)?;
            let debug_bundle = darwin_debug_artifact_path(&output);
            let command = [
                tool.as_os_str().to_owned(),
                output.into_os_string(),
                OsString::from("-o"),
                debug_bundle.into_os_string(),
            ];
            lines.push(render_shell_command(&command));
        }
    }

    Ok(DriverOutput {
        stdout: String::new(),
        stderr: format!("{}\n", lines.join("\n")),
    })
}

fn render_ccc_compile_command(
    executable: &Path,
    options: &DriverOptions,
    input: &Path,
    language: Option<DriverInputLanguage>,
    output: &Path,
) -> String {
    let mut command = vec![executable.as_os_str().to_owned(), "-c".into()];
    append_target_command_arguments(&mut command, options);
    command.push(match options.language_mode {
        ccc_target::LanguageMode::C11 => "-std=c11".into(),
        ccc_target::LanguageMode::Gnu11 => "-std=gnu11".into(),
    });
    command.push(match options.relocation_model {
        ccc_target::RelocationModel::Static => "-fno-pic".into(),
        ccc_target::RelocationModel::Pic => "-fPIC".into(),
        ccc_target::RelocationModel::Pie => "-fPIE".into(),
    });
    command.push(options.optimization.flag().into());
    if options.debug_info {
        command.push("-g".into());
    }
    if options.trigraphs == ccc_target::TrigraphPolicy::Enabled {
        command.push("-trigraphs".into());
    }
    if options.suppress_warnings {
        command.push("-w".into());
    }
    if options.warnings_as_errors {
        command.push("-Werror".into());
    }
    command.extend(options.warning_options.iter().map(OsString::from));
    if let Some(limit) = options.error_limit {
        command.push(format!("-ferror-limit={limit}").into());
    }
    if options.diagnostic_format == DiagnosticFormat::Json {
        command.push("-fdiagnostics-format=json".into());
    }
    command.extend(options.degraded_hardening.iter().map(OsString::from));
    if options.no_standard_includes {
        command.push("-nostdinc".into());
    }
    if options.no_builtin_includes {
        command.push("-nobuiltininc".into());
    }
    for action in &options.macro_actions {
        match action {
            MacroAction::Define(definition) => command.push(format!("-D{definition}").into()),
            MacroAction::Undefine(name) => command.push(format!("-U{name}").into()),
        }
    }
    for include in &options.include_paths {
        let spelling = match include.kind {
            DriverIncludePathKind::Quote => "-iquote",
            DriverIncludePathKind::User => "-I",
            DriverIncludePathKind::System => "-isystem",
            DriverIncludePathKind::After => "-idirafter",
        };
        command.push(spelling.into());
        command.push(include.path.as_os_str().to_owned());
    }
    for forced in &options.forced_inputs {
        command.push(match forced.kind {
            ForcedInputKind::Macros => "-imacros".into(),
            ForcedInputKind::Include => "-include".into(),
        });
        command.push(forced.path.as_os_str().to_owned());
    }
    match options.dependencies.mode {
        DriverDependencyMode::None | DriverDependencyMode::Only { .. } => {}
        DriverDependencyMode::SideEffect {
            include_system: true,
        } => command.push("-MD".into()),
        DriverDependencyMode::SideEffect {
            include_system: false,
        } => command.push("-MMD".into()),
    }
    if options.dependencies.phony_targets {
        command.push("-MP".into());
    }
    if let Some(output) = &options.dependencies.output {
        command.push("-MF".into());
        command.push(output.as_os_str().to_owned());
    }
    for target in &options.dependencies.targets {
        command.push(
            match target {
                DependencyTarget::Literal(_) => "-MT",
                DependencyTarget::Quoted(_) => "-MQ",
            }
            .into(),
        );
        command.push(
            match target {
                DependencyTarget::Literal(value) | DependencyTarget::Quoted(value) => value,
            }
            .into(),
        );
    }
    if let Some(language) = language {
        command.push("-x".into());
        command.push(
            match language {
                DriverInputLanguage::C => "c",
                DriverInputLanguage::PreprocessedC => "c-cpp-output",
                DriverInputLanguage::Assembly => "assembler",
                DriverInputLanguage::PreprocessedAssembly => "assembler-with-cpp",
            }
            .into(),
        );
    }
    command.push(input.as_os_str().to_owned());
    command.push("-o".into());
    command.push(output.as_os_str().to_owned());
    render_shell_command(&command)
}

fn append_target_command_arguments(command: &mut Vec<OsString>, options: &DriverOptions) {
    if let Some(target) = &options.target {
        command.push(format!("--target={target}").into());
    }
    if let Some(architecture) = &options.target_arch {
        command.push(format!("-march={architecture}").into());
    }
    if let Some(cpu) = &options.target_cpu {
        command.push(format!("-mcpu={cpu}").into());
    }
    if let Some(abi) = &options.target_abi {
        command.push(format!("-mabi={abi}").into());
    }
    if let Some(sysroot) = &options.sysroot {
        command.push(format!("--sysroot={}", sysroot.display()).into());
    }
    if let Some(resource_dir) = &options.resource_dir {
        command.push("-resource-dir".into());
        command.push(resource_dir.as_os_str().to_owned());
    }
    if let Some(sdk_root) = &options.sdk_root {
        command.push("--sdk-root".into());
        command.push(sdk_root.as_os_str().to_owned());
    }
    if let Some(version) = &options.deployment_target {
        command.push(format!("-mmacosx-version-min={version}").into());
    }
}

fn render_shell_command(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| shell_quote(&argument.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:=,@".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn syntax_only_output(prepared: PreparedSource) -> Result<DriverOutput, DriverError> {
    let (parsed, _) = analyze_frontend_preprocessed(prepared)?;
    Ok(DriverOutput {
        stdout: String::new(),
        stderr: parsed.stderr,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputKind {
    C,
    PreprocessedC,
    Assembly,
    PreprocessedAssembly,
    Linker,
}

fn classify_input(path: &Path, language: Option<DriverInputLanguage>) -> InputKind {
    if let Some(language) = language {
        return match language {
            DriverInputLanguage::C => InputKind::C,
            DriverInputLanguage::PreprocessedC => InputKind::PreprocessedC,
            DriverInputLanguage::Assembly => InputKind::Assembly,
            DriverInputLanguage::PreprocessedAssembly => InputKind::PreprocessedAssembly,
        };
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".so."))
    {
        return InputKind::Linker;
    }
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("i") => InputKind::PreprocessedC,
        Some("s") => InputKind::Assembly,
        Some("S") => InputKind::PreprocessedAssembly,
        Some("o" | "lo" | "obj" | "a" | "so" | "dylib") => InputKind::Linker,
        _ => InputKind::C,
    }
}

fn compile_multiple_outputs(options: DriverOptions) -> Result<DriverOutput, DriverError> {
    let mut stdout = String::new();
    let mut stderr = String::new();
    for (index, input) in options.inputs.iter().enumerate() {
        let language = options.input_languages[index];
        let output = compile_output_path(input);
        let result = match classify_input(input, language) {
            InputKind::C | InputKind::PreprocessedC => {
                let mut per_input = options.clone();
                per_input.input = input.clone();
                per_input.inputs = vec![input.clone()];
                per_input.input_languages = vec![language];
                per_input.link_items.clear();
                per_input.output = Some(output);
                preprocess_source(&per_input)
                    .and_then(|prepared| compile_output(&per_input, prepared, false))
            }
            InputKind::Assembly | InputKind::PreprocessedAssembly => {
                let mut per_input = options.clone();
                per_input.input = input.clone();
                per_input.inputs = vec![input.clone()];
                per_input.input_languages = vec![language];
                effective_config(&per_input).and_then(|(config, _)| {
                    assemble_input(&per_input, input, language, &output, &config)
                })
            }
            InputKind::Linker => Err(DriverError::new(format!(
                "ccc: `-c` input {} is already a linker input",
                input.display()
            ))),
        }
        .map_err(|error| with_prior_diagnostics(&stderr, error))?;
        stdout.push_str(&result.stdout);
        append_diagnostic_output(&mut stderr, &result.stderr, options.diagnostic_format);
    }
    Ok(DriverOutput { stdout, stderr })
}

fn compile_output_path(input: &Path) -> PathBuf {
    input
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| input.to_path_buf())
        .with_extension("o")
}

fn link_output(options: DriverOptions) -> Result<DriverOutput, DriverError> {
    let (config, _) = effective_config(&options)?;
    let output = options
        .output
        .clone()
        .unwrap_or_else(|| match options.link_output_kind {
            DriverLinkOutputKind::Executable => PathBuf::from("a.out"),
            DriverLinkOutputKind::Shared
                if config.target.triple.binary_format == ccc_target::BinaryFormat::Macho =>
            {
                PathBuf::from("a.dylib")
            }
            DriverLinkOutputKind::Shared => PathBuf::from("a.so"),
            DriverLinkOutputKind::Relocatable => PathBuf::from("a.o"),
        });
    let mut temporaries = Vec::new();
    let mut packaging_reports = Vec::new();
    let mut linker_inputs = Vec::<OsString>::new();
    let mut stdout = String::new();
    let mut stderr = String::new();

    for item in &options.link_items {
        match item {
            LinkItem::Argument(argument) => linker_inputs.push(OsString::from(argument.as_str())),
            LinkItem::Input {
                path: input,
                language,
            } => match classify_input(input, *language) {
                InputKind::C | InputKind::PreprocessedC => {
                    let temporary = TemporaryObject::create()
                        .map_err(|error| with_prior_diagnostics(&stderr, error))?;
                    let mut per_input = options.clone();
                    per_input.action = PrimaryAction::Compile { link: false };
                    per_input.input = input.clone();
                    per_input.inputs = vec![input.clone()];
                    per_input.input_languages = vec![*language];
                    per_input.link_items.clear();
                    per_input.output = Some(temporary.path().to_path_buf());
                    let prepared = preprocess_source(&per_input)
                        .map_err(|error| with_prior_diagnostics(&stderr, error))?;
                    let (result, packaging) =
                        compile_output_retaining_packaging(&per_input, prepared, false)
                            .map_err(|error| with_prior_diagnostics(&stderr, error))?;
                    packaging_reports.push(packaging);
                    stdout.push_str(&result.stdout);
                    append_diagnostic_output(
                        &mut stderr,
                        &result.stderr,
                        options.diagnostic_format,
                    );
                    linker_inputs.push(temporary.path().as_os_str().to_owned());
                    temporaries.push(temporary);
                }
                InputKind::Linker => linker_inputs.push(input.as_os_str().to_owned()),
                InputKind::Assembly | InputKind::PreprocessedAssembly => {
                    let mut temporary = TemporaryObject::create()
                        .map_err(|error| with_prior_diagnostics(&stderr, error))?;
                    let temporary_path = temporary
                        .prepare_external_write()
                        .map_err(|error| with_prior_diagnostics(&stderr, error))?
                        .to_path_buf();
                    let result =
                        assemble_input(&options, input, *language, &temporary_path, &config)
                            .map_err(|error| with_prior_diagnostics(&stderr, error))?;
                    stdout.push_str(&result.stdout);
                    append_diagnostic_output(
                        &mut stderr,
                        &result.stderr,
                        options.diagnostic_format,
                    );
                    linker_inputs.push(temporary.path().as_os_str().to_owned());
                    temporaries.push(temporary);
                }
            },
        }
    }

    let mut pending = atomic_output::PendingOutput::create(&output).map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot create output {}: {error}",
                output.display()
            )),
        )
    })?;
    let pending_path = pending.prepare_external_write().map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot prepare output {}: {error}",
                output.display()
            )),
        )
    })?;
    let kind = match options.link_output_kind {
        DriverLinkOutputKind::Executable => ccc_link::LinkOutputKind::Executable,
        DriverLinkOutputKind::Shared => ccc_link::LinkOutputKind::Shared,
        DriverLinkOutputKind::Relocatable => ccc_link::LinkOutputKind::Relocatable,
    };
    ccc_link::link_inputs_with_toolchain(
        &linker_inputs,
        pending_path,
        kind,
        &config,
        &config.toolchain,
    )
    .map_err(|error| with_prior_diagnostics(&stderr, owner_error(error.code, error.message)))?;
    pending.commit(&output).map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot replace output {}: {error}",
                output.display()
            )),
        )
    })?;
    if should_materialize_darwin_debug_artifact(&options, &config) {
        let debug_stderr = materialize_darwin_debug_artifact(&output, &config)
            .map_err(|error| with_prior_diagnostics(&stderr, error))?;
        append_diagnostic_output(&mut stderr, &debug_stderr, options.diagnostic_format);
    }
    drop(packaging_reports);
    drop(temporaries);
    Ok(DriverOutput { stdout, stderr })
}

fn should_materialize_darwin_debug_artifact(
    options: &DriverOptions,
    config: &EffectiveCompilationConfig,
) -> bool {
    options.debug_info
        && options.link_output_kind != DriverLinkOutputKind::Relocatable
        && config.target.triple.binary_format == ccc_target::BinaryFormat::Macho
}

fn darwin_debug_artifact_path(output: &Path) -> PathBuf {
    let mut path = output.as_os_str().to_owned();
    path.push(".dSYM");
    PathBuf::from(path)
}

fn resolve_dsymutil(config: &EffectiveCompilationConfig) -> Result<PathBuf, DriverError> {
    let driver =
        config.toolchain.compiler_driver.as_ref().ok_or_else(|| {
            owner_error("CCC6006", "no target compiler driver can resolve dsymutil")
        })?;
    let result = Command::new(&driver.program)
        .args(&driver.arguments)
        .arg("-print-prog-name=dsymutil")
        .output()
        .map_err(|error| {
            owner_error(
                "CCC6006",
                format!(
                    "cannot ask target compiler driver `{}` for dsymutil: {error}",
                    driver.display()
                ),
            )
        })?;
    if !result.status.success() {
        return Err(owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` could not resolve dsymutil: {}",
                driver.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            ),
        ));
    }
    let reported = String::from_utf8(result.stdout).map_err(|error| {
        owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` returned a non-UTF-8 dsymutil path: {error}",
                driver.display()
            ),
        )
    })?;
    let mut lines = reported.lines();
    let path = lines.next().map(str::trim).filter(|path| !path.is_empty());
    if path.is_none() || lines.any(|line| !line.trim().is_empty()) {
        return Err(owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` returned an invalid dsymutil path `{}`",
                driver.display(),
                reported.trim()
            ),
        ));
    }
    let path = PathBuf::from(path.expect("nonempty path was checked"));
    if !path.is_absolute() {
        return Err(owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` did not report an absolute dsymutil path: `{}`",
                driver.display(),
                path.display()
            ),
        ));
    }
    let metadata = fs::metadata(&path).map_err(|error| {
        owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` reported unusable dsymutil path `{}`: {error}",
                driver.display(),
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return Err(owner_error(
            "CCC6006",
            format!(
                "target compiler driver `{}` did not report an absolute executable dsymutil path: `{}`",
                driver.display(),
                path.display()
            ),
        ));
    }
    Ok(path)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn materialize_darwin_debug_artifact(
    output: &Path,
    config: &EffectiveCompilationConfig,
) -> Result<String, DriverError> {
    let tool = resolve_dsymutil(config)?;
    let destination = darwin_debug_artifact_path(output);
    let pending = atomic_output::PendingDirectory::create(&destination).map_err(|error| {
        owner_error(
            "CCC6006",
            format!(
                "cannot prepare debug artifact {}: {error}",
                destination.display()
            ),
        )
    })?;
    let result = Command::new(&tool)
        .arg(output)
        .arg("-o")
        .arg(pending.path())
        .output()
        .map_err(|error| {
            owner_error(
                "CCC6006",
                format!("cannot invoke dsymutil `{}`: {error}", tool.display()),
            )
        })?;
    if !result.status.success() {
        return Err(owner_error(
            "CCC6006",
            format!(
                "dsymutil `{}` failed for {}: {}",
                tool.display(),
                output.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            ),
        ));
    }
    pending.commit(&destination).map_err(|error| {
        owner_error(
            "CCC6006",
            format!(
                "cannot publish debug artifact {}: {error}",
                destination.display()
            ),
        )
    })?;
    let mut stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    if !stderr.is_empty() && !stderr.ends_with('\n') {
        stderr.push('\n');
    }
    Ok(stderr)
}

fn assemble_input(
    options: &DriverOptions,
    input: &Path,
    language: Option<DriverInputLanguage>,
    output: &Path,
    config: &EffectiveCompilationConfig,
) -> Result<DriverOutput, DriverError> {
    let mut stderr = String::new();
    let preprocessed;
    let assembly = if classify_input(input, language) == InputKind::PreprocessedAssembly {
        let mut per_input = options.clone();
        per_input.action = PrimaryAction::Preprocess;
        per_input.input = input.to_path_buf();
        per_input.inputs = vec![input.to_path_buf()];
        per_input.link_items.clear();
        per_input.output = None;
        let prepared = preprocess_source(&per_input)?;
        prepared.reject_if_errors()?;
        append_diagnostic_output(&mut stderr, &prepared.stderr, options.diagnostic_format);
        let source = render_preprocessed(&prepared.output, false);
        preprocessed = Some(
            TemporaryAssembly::create(source.as_bytes())
                .map_err(|error| with_prior_diagnostics(&stderr, error))?,
        );
        preprocessed.as_ref().expect("temporary was created").path()
    } else {
        preprocessed = None;
        input
    };
    let driver = config.toolchain.compiler_driver.as_ref().ok_or_else(|| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new("ccc: resolved toolchain has no compiler driver"),
        )
    })?;
    let mut pending = atomic_output::PendingOutput::create(output).map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot create output {}: {error}",
                output.display()
            )),
        )
    })?;
    let pending_path = pending.prepare_external_write().map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot prepare output {}: {error}",
                output.display()
            )),
        )
    })?;
    let result = Command::new(&driver.program)
        .args(&driver.arguments)
        .arg("-x")
        .arg("assembler")
        .arg("-c")
        .arg(assembly)
        .arg("-o")
        .arg(pending_path)
        .output()
        .map_err(|error| {
            with_prior_diagnostics(
                &stderr,
                DriverError::new(format!(
                    "ccc: cannot invoke target compiler driver `{}` for assembly input: {error}",
                    driver.display()
                )),
            )
        })?;
    drop(preprocessed);
    if !result.status.success() {
        return Err(with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: target compiler driver `{}` failed to assemble {}: {}",
                driver.display(),
                input.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            )),
        ));
    }
    pending.commit(output).map_err(|error| {
        with_prior_diagnostics(
            &stderr,
            DriverError::new(format!(
                "ccc: cannot replace output {}: {error}",
                output.display()
            )),
        )
    })?;
    Ok(DriverOutput {
        stdout: String::new(),
        stderr,
    })
}

struct PreparedSource {
    session: Session,
    output: PreprocessOutput,
    diagnostics: DiagnosticEngine,
    diagnostic_format: DiagnosticFormat,
    stderr: String,
}

impl PreparedSource {
    fn reject_if_errors(&self) -> Result<(), DriverError> {
        if self.diagnostics.has_errors() {
            Err(diagnostic_engine_error(
                &self.session.sources,
                &self.diagnostics,
                self.diagnostic_format,
            ))
        } else {
            Ok(())
        }
    }
}

fn preprocess_source(options: &DriverOptions) -> Result<PreparedSource, DriverError> {
    let (config, resources) = effective_config(options)?;
    let provider = FsFileProvider;
    let loaded = provider.read(&options.input).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::InvalidData {
            "CCC6002"
        } else {
            "CCC6001"
        };
        owner_error(
            code,
            format!("cannot read {}: {error}", options.input.display()),
        )
    })?;

    let mut session = Session::new(config);
    let main_spec = SourceFileSpec::new(&options.input)
        .with_display_name(options.input.to_string_lossy())
        .with_resolved_path(&loaded.path)
        .with_identity(ccc_session::FileIdentity::Opaque(loaded.identity.0.clone()));
    let main_file = session
        .sources
        .add_file_occurrence(main_spec, loaded.source);
    let pp_options = preprocessing_options(options, &session.config, resources.as_ref());
    let warning_policy = WarningPolicy::new(
        options.suppress_warnings,
        options.warnings_as_errors,
        &options.warning_options,
    );
    let warn_in_system_headers = warning_policy.enabled("system-headers");
    let mut sink = PreprocessorDiagnostics::new(
        &warning_policy,
        !options.suppress_warnings,
        options.warnings_as_errors,
        warn_in_system_headers,
        options.error_limit.unwrap_or(20),
    );
    let output = preprocess(
        &mut PreprocessContext {
            session: &mut session,
            diagnostics: &mut sink,
            options: &pp_options,
            files: &provider,
        },
        main_file,
    );

    let diagnostics = sink.finish();
    debug_assert!(!output.had_errors || diagnostics.has_errors());
    let stderr = successful_diagnostics(&session.sources, &diagnostics, options.diagnostic_format);

    if !diagnostics.has_errors()
        && let Some(header) = unsupported_hosted_header(&options.action, &session.config, &output)
    {
        let required = required_compatibility_scope(&options.action);
        let available = session
            .config
            .gnu_profile
            .as_ref()
            .map_or(CompatibilityScope::Preprocessing, |profile| profile.scope);
        return Err(with_prior_diagnostics(
            &render_diagnostics_for_error(
                &session.sources,
                &diagnostics,
                options.diagnostic_format,
            ),
            owner_error(
                "CCC6004",
                format!(
                    "cannot continue through {} after selecting hosted header {}: GNU profile {} is certified through {}",
                    compatibility_scope_name(required),
                    header.display(),
                    session
                        .config
                        .gnu_profile
                        .as_ref()
                        .map_or("<none>", |profile| profile.name.as_str()),
                    compatibility_scope_name(available),
                ),
            ),
        ));
    }

    Ok(PreparedSource {
        session,
        output,
        diagnostics,
        diagnostic_format: options.diagnostic_format,
        stderr,
    })
}

fn effective_config(
    options: &DriverOptions,
) -> Result<(EffectiveCompilationConfig, Option<ResourceDirectory>), DriverError> {
    let mut config = if let Some(target) = &options.target {
        let triple = target.parse().map_err(|error| {
            owner_error(
                "CCC6005",
                format!("invalid target triple `{target}`: {error}"),
            )
        })?;
        EffectiveCompilationConfig::for_target(triple)
            .map_err(|message| owner_error("CCC6005", message))?
    } else {
        EffectiveCompilationConfig::host().map_err(|message| owner_error("CCC6005", message))?
    }
    .with_language_mode(options.language_mode);
    config.language.trigraphs = options.trigraphs;
    if let Some(architecture) = &options.target_arch {
        config = config.with_target_arch(architecture);
    }
    if let Some(cpu) = &options.target_cpu {
        config = config.with_target_cpu(cpu);
    }
    if let Some(abi) = &options.target_abi {
        config = config.with_target_abi(abi);
    }
    if let Some(sdk_root) = &options.sdk_root {
        config = config.with_sdk_root(sdk_root);
    }
    if let Some(version) = &options.deployment_target {
        config = config.with_deployment_target(version);
    }
    if options.sdk_root.is_some() && config.target.abi != ccc_target::AbiIdentity::DarwinArm64 {
        return Err(owner_error(
            "CCC6005",
            "`--sdk-root` is valid only for the Darwin arm64 target",
        ));
    }
    if options.deployment_target.is_some()
        && config.target.abi != ccc_target::AbiIdentity::DarwinArm64
    {
        return Err(owner_error(
            "CCC6005",
            "`-mmacosx-version-min` is valid only for the Darwin arm64 target",
        ));
    }
    config.relocation_model = options.relocation_model;
    config.optimization = options.optimization;
    config
        .validate_target_profile_options()
        .map_err(|message| owner_error("CCC6005", message))?;

    let should_load_resources = options.resource_dir.is_some()
        || (!options.no_standard_includes && !options.no_builtin_includes);
    let resources = should_load_resources
        .then(|| ResourceDirectory::discover(options.resource_dir.as_deref()))
        .transpose()
        .map_err(|message| owner_error("CCC6003", message))?;
    if let Some(resources) = &resources {
        if config.gnu_profile.as_ref() != Some(resources.hosted_header_profile()) {
            return Err(owner_error(
                "CCC6003",
                format!(
                    "resource profile {} does not match the effective GNU compatibility profile",
                    resources.hosted_header_profile().name
                ),
            ));
        }
        if !options.no_standard_includes && !options.no_builtin_includes {
            config.resource_dir = Some(resources.root().to_path_buf());
        }
    }

    let link = matches!(options.action, PrimaryAction::Compile { link: true });
    let effective_sysroot = options.sysroot.as_ref().or(options.sdk_root.as_ref());
    let resolve_system_headers =
        !options.no_standard_includes && should_probe_native_toolchain(&config);
    if resolve_system_headers || link || effective_sysroot.is_some() {
        let mut resolver = ToolchainResolver::new(&config);
        if let Some(sysroot) = effective_sysroot {
            resolver = resolver.sysroot(sysroot);
        }
        let requirements = ToolchainRequirements {
            system_headers: resolve_system_headers,
            disable_system_headers: options.no_standard_includes,
            assembler: options.inputs.iter().enumerate().any(|(index, input)| {
                matches!(
                    classify_input(input, options.input_languages[index]),
                    InputKind::Assembly | InputKind::PreprocessedAssembly
                )
            }),
            linker: link,
            object_copier: false,
            archiver: false,
        };
        let mut toolchain = resolver
            .resolve(requirements)
            .map_err(|error| owner_error(error.code, error.message))?;
        if options.no_builtin_includes {
            toolchain
                .system_includes
                .retain(|entry| entry.kind != SystemIncludeKind::Builtin);
        }
        config = config.with_toolchain(toolchain);
    }

    Ok((config, resources))
}

fn should_probe_native_toolchain(config: &EffectiveCompilationConfig) -> bool {
    let _ = config;
    true
}

fn unsupported_hosted_header<'a>(
    action: &PrimaryAction,
    config: &EffectiveCompilationConfig,
    output: &'a PreprocessOutput,
) -> Option<&'a Path> {
    let required = required_compatibility_scope(action);
    let available = config
        .gnu_profile
        .as_ref()
        .map_or(CompatibilityScope::Preprocessing, |profile| profile.scope);
    if available.includes(required) {
        return None;
    }

    output
        .dependencies
        .edges
        .iter()
        .map(|edge| edge.to.as_path())
        .chain(output.items.iter().filter_map(|item| match item {
            PpItem::LineMarker(marker) if marker.flags.contains(&3) => {
                Some(Path::new(&marker.file))
            }
            _ => None,
        }))
        .find(|path| is_toolchain_header(config, path))
}

fn required_compatibility_scope(action: &PrimaryAction) -> CompatibilityScope {
    match action {
        PrimaryAction::Preprocess | PrimaryAction::Dump(DumpKind::PpTokens) => {
            CompatibilityScope::Preprocessing
        }
        PrimaryAction::Dump(DumpKind::Tokens | DumpKind::Ast) => CompatibilityScope::Parsing,
        PrimaryAction::SyntaxOnly | PrimaryAction::Dump(DumpKind::TypedAst) => {
            CompatibilityScope::SemanticAnalysis
        }
        PrimaryAction::Compile { .. }
        | PrimaryAction::Dump(
            DumpKind::Ir | DumpKind::Abi | DumpKind::Clif | DumpKind::CodegenStats,
        ) => CompatibilityScope::CodeGeneration,
    }
}

const fn compatibility_scope_name(scope: CompatibilityScope) -> &'static str {
    match scope {
        CompatibilityScope::Preprocessing => "preprocessing",
        CompatibilityScope::Parsing => "parsing",
        CompatibilityScope::SemanticAnalysis => "semantic analysis",
        CompatibilityScope::CodeGeneration => "code generation",
    }
}

fn is_toolchain_header(config: &EffectiveCompilationConfig, path: &Path) -> bool {
    let compiler_resource_include = config
        .resource_dir
        .as_deref()
        .map(|directory| directory.join("include"));
    if compiler_resource_include
        .as_deref()
        .is_some_and(|directory| path_is_within(path, directory))
    {
        return false;
    }
    config
        .system_includes()
        .iter()
        .any(|entry| path_is_within(path, &entry.path))
}

fn path_is_within(path: &Path, directory: &Path) -> bool {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let directory = fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    path.starts_with(directory)
}

fn preprocessing_options(
    options: &DriverOptions,
    config: &EffectiveCompilationConfig,
    resources: Option<&ResourceDirectory>,
) -> PreprocessOptions {
    let mut include_paths = options
        .include_paths
        .iter()
        .map(|entry| {
            IncludePath::new(
                &entry.path,
                match entry.kind {
                    DriverIncludePathKind::Quote => IncludePathKind::Quote,
                    DriverIncludePathKind::User => IncludePathKind::User,
                    DriverIncludePathKind::System => IncludePathKind::System,
                    DriverIncludePathKind::After => IncludePathKind::After,
                },
            )
        })
        .collect::<Vec<_>>();
    if !options.no_standard_includes
        && !options.no_builtin_includes
        && let Some(resources) = resources
    {
        include_paths.push(IncludePath::new(
            resources.include(),
            IncludePathKind::Resource,
        ));
    }

    let command_line_macros = options
        .macro_actions
        .iter()
        .map(|action| match action {
            MacroAction::Define(definition) => CommandLineMacro::Define(definition.clone()),
            MacroAction::Undefine(name) => CommandLineMacro::Undefine(name.clone()),
        })
        .collect();
    let imacros = options
        .forced_inputs
        .iter()
        .filter(|input| input.kind == ForcedInputKind::Macros)
        .map(|input| input.path.clone())
        .collect();
    let forced_includes = options
        .forced_inputs
        .iter()
        .filter(|input| input.kind == ForcedInputKind::Include)
        .map(|input| input.path.clone())
        .collect();
    let (date_macro, time_macro) = predefined::translation_date_and_time();
    let limits = PreprocessLimits {
        diagnostics: usize::MAX,
        ..PreprocessLimits::default()
    };

    PreprocessOptions {
        language_mode: options.language_mode,
        preprocessed_input: classify_input(
            &options.input,
            options.input_languages.first().copied().flatten(),
        ) == InputKind::PreprocessedC,
        trigraphs: Some(config.language.trigraphs_enabled()),
        warn_trigraphs: true,
        include_paths,
        command_line_macros,
        imacros,
        forced_includes,
        predefined_macros: predefined::additional_predefined_macros(config),
        features: predefined::feature_predicates(config),
        dependency_mode: match options.dependencies.mode {
            DriverDependencyMode::None => DependencyMode::None,
            DriverDependencyMode::Only {
                include_system: true,
            } => DependencyMode::All,
            DriverDependencyMode::Only {
                include_system: false,
            } => DependencyMode::User,
            DriverDependencyMode::SideEffect {
                include_system: true,
            } => DependencyMode::SideEffectAll,
            DriverDependencyMode::SideEffect {
                include_system: false,
            } => DependencyMode::SideEffectUser,
        },
        suppress_line_markers: options.suppress_linemarkers,
        preserve_comments: false,
        gnu_comma_elision: options.language_mode.accepts_gnu_extensions(),
        limits,
        date_macro,
        time_macro,
    }
}

fn dependency_only_output(
    options: &DriverOptions,
    prepared: PreparedSource,
) -> Result<DriverOutput, DriverError> {
    prepared.reject_if_errors()?;
    let rendered = rendered_dependencies(options, &prepared.output, None);
    let destination = options
        .dependencies
        .output
        .as_ref()
        .or(options.output.as_ref());
    let stdout = if let Some(destination) = destination.filter(|path| *path != Path::new("-")) {
        atomic_output::write_atomic(destination, rendered.as_bytes()).map_err(|error| {
            with_prior_diagnostics(
                &prepared.stderr,
                DriverError::new(format!(
                    "ccc: cannot write dependency file {}: {error}",
                    destination.display()
                )),
            )
        })?;
        String::new()
    } else {
        rendered.contents
    };
    Ok(DriverOutput {
        stdout,
        stderr: prepared.stderr,
    })
}

fn preprocess_output(
    options: &DriverOptions,
    prepared: PreparedSource,
) -> Result<DriverOutput, DriverError> {
    prepared.reject_if_errors()?;
    let mut stdout = if options.dump_macros {
        render_macro_definitions(&prepared.output.macros)
    } else {
        render_preprocessed(&prepared.output, options.suppress_linemarkers)
    };

    let dependency_stdout = if matches!(
        options.dependencies.mode,
        DriverDependencyMode::SideEffect { .. }
    ) {
        let dependency_target = options
            .dependencies
            .output
            .as_ref()
            .and(options.output.as_deref());
        write_side_effect_dependencies(options, &prepared.output, dependency_target)
            .map_err(|error| with_prior_diagnostics(&prepared.stderr, error))?
    } else {
        String::new()
    };

    let output_is_dependency_destination = matches!(
        options.dependencies.mode,
        DriverDependencyMode::SideEffect { .. }
    ) && options.dependencies.output.is_none()
        && options.output.is_some();
    if !output_is_dependency_destination && let Some(output) = &options.output {
        atomic_output::write_atomic(output, stdout.as_bytes()).map_err(|error| {
            with_prior_diagnostics(
                &prepared.stderr,
                DriverError::new(format!("ccc: cannot write {}: {error}", output.display())),
            )
        })?;
        stdout.clear();
    }
    stdout.push_str(&dependency_stdout);

    Ok(DriverOutput {
        stdout,
        stderr: prepared.stderr,
    })
}

fn dump_output(
    options: &DriverOptions,
    prepared: PreparedSource,
    kind: DumpKind,
) -> Result<DriverOutput, DriverError> {
    let deferred_dependencies = defer_side_effect_dependencies(options, &prepared.output, None);

    let (mut stdout, stderr) = match kind {
        DumpKind::PpTokens => {
            prepared.reject_if_errors()?;
            (
                dump_pp_tokens(&prepared.session.sources, &prepared.output.tokens()),
                prepared.stderr,
            )
        }
        DumpKind::Tokens => {
            let mut prepared = prepared;
            prepared.reject_if_errors()?;
            let items = match convert_pp_items(prepared.output.items) {
                Ok(items) => items,
                Err(error) => {
                    let _ = prepared.diagnostics.emit(
                        &prepared.session.sources,
                        Diagnostic::error(error.code, error.message)
                            .with_category("lexical")
                            .with_primary(error.span, "while converting preprocessing tokens"),
                    );
                    return Err(diagnostic_engine_error(
                        &prepared.session.sources,
                        &prepared.diagnostics,
                        prepared.diagnostic_format,
                    ));
                }
            };
            (
                dump_frontend_tokens(&prepared.session.sources, &items),
                prepared.stderr,
            )
        }
        DumpKind::Ast => {
            let mut parsed = parse_frontend_preprocessed(prepared)?;
            parsed.reject_if_errors()?;
            (dump_frontend_ast(&parsed.ast), parsed.stderr)
        }
        DumpKind::TypedAst => {
            let (parsed, typed) = analyze_frontend_preprocessed(prepared)?;
            (dump_frontend_typed_ast(&typed), parsed.stderr)
        }
        DumpKind::Ir => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared)?;
            (ccc_ir::generic::dump_frontend_ir(&ir), parsed.stderr)
        }
        DumpKind::Abi => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared)?;
            let plan = ccc_abi::plan_module(&ir, &parsed.session.config).map_err(|error| {
                with_prior_diagnostics(
                    &parsed.stderr,
                    abi_driver_error(&parsed.session.sources, error),
                )
            })?;
            let verified = plan
                .verify_against(&ir, &parsed.session.config)
                .map_err(|error| {
                    with_prior_diagnostics(
                        &parsed.stderr,
                        abi_driver_error(&parsed.session.sources, error),
                    )
                })?;
            (ccc_abi::dump_module_plan(verified), parsed.stderr)
        }
        DumpKind::Clif => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared)?;
            let generated = codegen_frontend(
                &ir,
                &parsed.session.config,
                &parsed.session.sources,
                true,
                false,
                false,
            )
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
            (generated.clif, parsed.stderr)
        }
        DumpKind::CodegenStats => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared)?;
            let generated = codegen_frontend(
                &ir,
                &parsed.session.config,
                &parsed.session.sources,
                false,
                options.debug_info,
                true,
            )
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
            (
                generated
                    .stats
                    .expect("codegen statistics were requested")
                    .to_tsv(),
                parsed.stderr,
            )
        }
    };
    let dependency_stdout = publish_deferred_dependencies(deferred_dependencies)
        .map_err(|error| with_prior_diagnostics(&stderr, error))?;
    stdout.push_str(&dependency_stdout);
    Ok(DriverOutput { stdout, stderr })
}

fn compile_output(
    options: &DriverOptions,
    prepared: PreparedSource,
    link: bool,
) -> Result<DriverOutput, DriverError> {
    let (output, _packaging) = compile_output_retaining_packaging(options, prepared, link)?;
    Ok(output)
}

fn compile_output_retaining_packaging(
    options: &DriverOptions,
    prepared: PreparedSource,
    link: bool,
) -> Result<(DriverOutput, ccc_link::PackagingReport), DriverError> {
    let output = options.output.clone().unwrap_or_else(|| {
        if link {
            PathBuf::from("a.out")
        } else {
            compile_output_path(&options.input)
        }
    });
    let deferred_dependencies =
        defer_side_effect_dependencies(options, &prepared.output, options.output.as_deref());

    let (parsed, ir) = lower_frontend_preprocessed(prepared)?;
    let generated = codegen_frontend(
        &ir,
        &parsed.session.config,
        &parsed.session.sources,
        false,
        options.debug_info,
        false,
    )
    .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
    let artifact = generated.into_artifact_bundle();
    let dependency_stdout;
    let packaging;
    if link {
        let mut temporary = TemporaryObject::create()
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
        let artifact_path = temporary
            .prepare_external_write()
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?
            .to_path_buf();
        packaging =
            ccc_link::package_artifact_bundle(artifact, &artifact_path, &parsed.session.config)
                .map_err(|error| {
                    with_prior_diagnostics(&parsed.stderr, owner_error(error.code, error.message))
                })?;
        let mut pending = atomic_output::PendingOutput::create(&output).map_err(|error| {
            with_prior_diagnostics(
                &parsed.stderr,
                DriverError::new(format!(
                    "ccc: cannot create output {}: {error}",
                    output.display()
                )),
            )
        })?;
        let link_output = pending
            .prepare_external_write()
            .map_err(|error| {
                with_prior_diagnostics(
                    &parsed.stderr,
                    DriverError::new(format!(
                        "ccc: cannot prepare output {}: {error}",
                        output.display()
                    )),
                )
            })?
            .to_path_buf();
        ccc_link::link_executable_with_toolchain(
            temporary.path(),
            &link_output,
            &parsed.session.config,
            &parsed.session.config.toolchain,
        )
        .map_err(|error| {
            with_prior_diagnostics(&parsed.stderr, owner_error(error.code, error.message))
        })?;
        dependency_stdout = publish_deferred_dependencies(deferred_dependencies)
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
        pending.commit(&output).map_err(|error| {
            with_prior_diagnostics(
                &parsed.stderr,
                DriverError::new(format!(
                    "ccc: cannot replace output {}: {error}",
                    output.display()
                )),
            )
        })?;
    } else {
        let mut pending = atomic_output::PendingOutput::create(&output).map_err(|error| {
            with_prior_diagnostics(
                &parsed.stderr,
                DriverError::new(format!(
                    "ccc: cannot create output {}: {error}",
                    output.display()
                )),
            )
        })?;
        let package_output = pending
            .prepare_external_write()
            .map_err(|error| {
                with_prior_diagnostics(
                    &parsed.stderr,
                    DriverError::new(format!(
                        "ccc: cannot prepare output {}: {error}",
                        output.display()
                    )),
                )
            })?
            .to_path_buf();
        packaging =
            ccc_link::package_artifact_bundle(artifact, &package_output, &parsed.session.config)
                .map_err(|error| {
                    with_prior_diagnostics(&parsed.stderr, owner_error(error.code, error.message))
                })?;
        dependency_stdout = publish_deferred_dependencies(deferred_dependencies)
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
        pending.commit(&output).map_err(|error| {
            with_prior_diagnostics(
                &parsed.stderr,
                DriverError::new(format!(
                    "ccc: cannot replace output {}: {error}",
                    output.display()
                )),
            )
        })?;
    }
    Ok((
        DriverOutput {
            stdout: dependency_stdout,
            stderr: parsed.stderr,
        },
        packaging,
    ))
}

fn rendered_dependencies(
    options: &DriverOptions,
    output: &PreprocessOutput,
    output_target: Option<&Path>,
) -> dependency::RenderedDependencies {
    let records = output
        .dependencies
        .files
        .iter()
        .map(|dependency| DependencyRecord::new(&dependency.path, dependency.system))
        .collect::<Vec<_>>();
    let targets = options
        .dependencies
        .targets
        .iter()
        .map(|target| match target {
            DependencyTarget::Literal(target) => MakeTarget::Literal(target.clone()),
            DependencyTarget::Quoted(target) => MakeTarget::Quoted(target.clone()),
        })
        .collect::<Vec<_>>();
    let include_system_headers = match options.dependencies.mode {
        DriverDependencyMode::Only { include_system }
        | DriverDependencyMode::SideEffect { include_system } => include_system,
        DriverDependencyMode::None => true,
    };
    render_dependencies(
        DependencyRenderOptions {
            main_source: &options.input,
            output_target,
            targets: &targets,
            include_system_headers,
            phony_targets: options.dependencies.phony_targets,
        },
        &records,
    )
}

struct DeferredDependencyPublication {
    rendered: dependency::RenderedDependencies,
    destination: PathBuf,
}

fn defer_side_effect_dependencies(
    options: &DriverOptions,
    output: &PreprocessOutput,
    output_target: Option<&Path>,
) -> Option<DeferredDependencyPublication> {
    if !matches!(
        options.dependencies.mode,
        DriverDependencyMode::SideEffect { .. }
    ) {
        return None;
    }
    let rendered = rendered_dependencies(options, output, output_target);
    let destination = options
        .dependencies
        .output
        .clone()
        .or_else(|| {
            if matches!(options.action, PrimaryAction::Preprocess) {
                options.output.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| default_dependency_path(&options.input, output_target));
    Some(DeferredDependencyPublication {
        rendered,
        destination,
    })
}

fn publish_deferred_dependencies(
    publication: Option<DeferredDependencyPublication>,
) -> Result<String, DriverError> {
    let Some(publication) = publication else {
        return Ok(String::new());
    };
    if publication.destination == Path::new("-") {
        return Ok(publication.rendered.contents);
    }
    atomic_output::write_atomic(&publication.destination, publication.rendered.as_bytes())
        .map_err(|error| {
            DriverError::new(format!(
                "ccc: cannot write dependency file {}: {error}",
                publication.destination.display()
            ))
        })
        .map(|()| String::new())
}

fn write_side_effect_dependencies(
    options: &DriverOptions,
    output: &PreprocessOutput,
    output_target: Option<&Path>,
) -> Result<String, DriverError> {
    publish_deferred_dependencies(defer_side_effect_dependencies(
        options,
        output,
        output_target,
    ))
}

struct FrontendParsedSource {
    session: Session,
    ast: FrontendTranslationUnit,
    diagnostics: DiagnosticEngine,
    diagnostic_format: DiagnosticFormat,
    stderr: String,
    poisoned_bindings: Vec<PoisonedBinding>,
}

impl FrontendParsedSource {
    fn reject_if_errors(&mut self) -> Result<(), DriverError> {
        if self.diagnostics.has_errors() {
            return Err(diagnostic_engine_error(
                &self.session.sources,
                &self.diagnostics,
                self.diagnostic_format,
            ));
        }
        self.stderr = successful_diagnostics(
            &self.session.sources,
            &self.diagnostics,
            self.diagnostic_format,
        );
        Ok(())
    }
}

fn parse_frontend_preprocessed(
    prepared: PreparedSource,
) -> Result<FrontendParsedSource, DriverError> {
    let PreparedSource {
        session,
        output,
        mut diagnostics,
        diagnostic_format,
        stderr,
    } = prepared;
    if diagnostics.is_halted() {
        return Err(diagnostic_engine_error(
            &session.sources,
            &diagnostics,
            diagnostic_format,
        ));
    }
    let items = match convert_pp_items(output.items) {
        Ok(items) => items,
        Err(error) => {
            let _ = diagnostics.emit(
                &session.sources,
                Diagnostic::error(error.code, error.message)
                    .with_category("lexical")
                    .with_primary(error.span, "while converting preprocessing tokens"),
            );
            return Err(diagnostic_engine_error(
                &session.sources,
                &diagnostics,
                diagnostic_format,
            ));
        }
    };
    let remaining = remaining_error_budget(&diagnostics);
    let recovered = parse_frontend_recovering_with_mode_and_limit(
        &items,
        session.config.language.mode,
        remaining,
    );
    for error in recovered.diagnostics {
        let outcome = diagnostics.emit(
            &session.sources,
            Diagnostic::error(error.code, error.message)
                .with_category("syntax")
                .with_primary(error.span, "while parsing"),
        );
        if outcome == EmitOutcome::Halted || diagnostics.is_halted() {
            break;
        }
    }
    Ok(FrontendParsedSource {
        session,
        ast: recovered.ast,
        diagnostics,
        diagnostic_format,
        stderr,
        poisoned_bindings: recovered.poisoned_bindings,
    })
}

fn analyze_frontend_preprocessed(
    prepared: PreparedSource,
) -> Result<(FrontendParsedSource, FullTypedTranslationUnit), DriverError> {
    let mut parsed = parse_frontend_preprocessed(prepared)?;
    let typed = if parsed.diagnostics.is_halted() {
        None
    } else {
        let remaining = remaining_error_budget(&parsed.diagnostics);
        match analyze_frontend_with_recovery_limit(
            &parsed.ast,
            &parsed.session.config,
            remaining,
            &parsed.poisoned_bindings,
        ) {
            Ok(typed) => Some(typed),
            Err(diagnostics) => {
                for mut diagnostic in diagnostics {
                    if diagnostic_depends_on_recovery(
                        &diagnostic,
                        &parsed.poisoned_bindings,
                        &parsed.session.sources,
                    ) {
                        continue;
                    }
                    if diagnostic.category.is_none() {
                        diagnostic = diagnostic.with_category("semantic");
                    }
                    let outcome = parsed.diagnostics.emit(&parsed.session.sources, diagnostic);
                    if outcome == EmitOutcome::Halted || parsed.diagnostics.is_halted() {
                        break;
                    }
                }
                None
            }
        }
    };
    parsed.reject_if_errors()?;
    let typed = typed.expect("error-free semantic analysis returns a typed translation unit");
    Ok((parsed, typed))
}

fn remaining_error_budget(diagnostics: &DiagnosticEngine) -> Option<usize> {
    let limit = diagnostics.options().error_limit;
    (limit != 0).then(|| limit.saturating_sub(diagnostics.error_count()))
}

fn diagnostic_depends_on_recovery(
    diagnostic: &Diagnostic,
    bindings: &[PoisonedBinding],
    sources: &SourceMap,
) -> bool {
    if diagnostic.code != "CCC2274" {
        return false;
    }
    let Some(primary) = diagnostic.primary.as_ref() else {
        return false;
    };
    let use_span = primary.span;
    let Some(spelling) = sources
        .source(use_span.file)
        .and_then(|source| source.get(use_span.start..use_span.end))
    else {
        return false;
    };
    bindings.iter().any(|binding| {
        binding.name == spelling
            && binding.binding.file == use_span.file
            && use_span.start >= binding.binding.end
            && use_span.start < binding.scope_end
    })
}

fn lower_frontend_preprocessed(
    prepared: PreparedSource,
) -> Result<(FrontendParsedSource, FullModule), DriverError> {
    let (parsed, typed) = analyze_frontend_preprocessed(prepared)?;
    let mut ir = ccc_ir::generic::lower_frontend(&typed).map_err(|error| {
        with_prior_diagnostics(
            &parsed.stderr,
            frontend_ir_error(&parsed.session.sources, error),
        )
    })?;
    ccc_ir::generic::optimize_frontend_for_config(&mut ir, &parsed.session.config).map_err(
        |error| {
            with_prior_diagnostics(
                &parsed.stderr,
                frontend_ir_error(&parsed.session.sources, error),
            )
        },
    )?;
    Ok((parsed, ir))
}

fn frontend_ir_error(sources: &SourceMap, error: FrontendIrError) -> DriverError {
    let mut diagnostic = Diagnostic::error(error.code, error.message);
    if let Some(span) = error.span {
        diagnostic = diagnostic.with_primary(span, "while lowering typed C to IR");
    }
    diagnostic_error(sources, diagnostic)
}

fn abi_driver_error(sources: &SourceMap, error: ccc_abi::AbiError) -> DriverError {
    let mut diagnostic = Diagnostic::error(error.code, error.message);
    if let Some(span) = error.span {
        diagnostic = diagnostic.with_primary(span, "while planning the native ABI boundary");
    }
    diagnostic_error(sources, diagnostic)
}

fn codegen_frontend(
    ir: &FullModule,
    config: &EffectiveCompilationConfig,
    sources: &SourceMap,
    emit_clif: bool,
    debug_info: bool,
    emit_stats: bool,
) -> Result<CodegenOutput, DriverError> {
    let codegen_options = CodegenOptions {
        emit_clif,
        debug_info: debug_info.then_some(sources),
    };
    let generated = if emit_stats {
        ccc_codegen::generic::emit_with_stats(ir, config, codegen_options)
    } else {
        ccc_codegen::generic::emit(ir, config, codegen_options)
    };
    generated.map_err(|error| {
        let mut diagnostic = Diagnostic::error(error.code, error.message);
        if let Some(span) = error.span {
            diagnostic = diagnostic.with_primary(span, "while generating native code");
        }
        diagnostic_error(sources, diagnostic)
    })
}

fn dump_pp_tokens(sources: &SourceMap, tokens: &[PpToken]) -> String {
    let mut stdout = String::new();
    for token in tokens {
        let location = sources
            .presumed_location(token.span.file, token.span.start)
            .expect("preprocessor spans source boundaries");
        let origin = if token.span.origin.is_direct() {
            "direct".to_owned()
        } else {
            let trace = sources.origin_trace(token.span.origin, 8);
            let summary = trace
                .frames
                .iter()
                .map(|origin| match &origin.kind {
                    ccc_session::OriginKind::MacroExpansion { macro_name, .. } => {
                        format!("macro:{macro_name}")
                    }
                    ccc_session::OriginKind::ArgumentSubstitution { parameter, .. } => {
                        format!("argument:{parameter}")
                    }
                    ccc_session::OriginKind::Stringization { .. } => "stringize".to_owned(),
                    ccc_session::OriginKind::TokenPaste { .. } => "paste".to_owned(),
                })
                .collect::<Vec<_>>()
                .join(">");
            if trace.truncated {
                format!("{summary}>...")
            } else {
                summary
            }
        };
        stdout.push_str(&format!(
            "{} {:?} {}:{}:{} origin={}\n",
            token.kind.as_str(),
            token.spelling,
            location.file_name,
            location.line,
            location.column,
            origin
        ));
    }
    stdout
}

fn dump_frontend_tokens(sources: &SourceMap, items: &[FrontendItem]) -> String {
    let mut stdout = String::new();
    for item in items {
        let FrontendItem::Token(token) = item else {
            continue;
        };
        let start = sources
            .presumed_location(token.span.file, token.span.start)
            .expect("token spans source boundaries");
        let end = sources
            .presumed_location(token.span.file, token.span.end)
            .expect("token spans source boundaries");
        let kind = match &token.kind {
            FrontendTokenKind::Keyword(_) => "keyword",
            FrontendTokenKind::Identifier => "identifier",
            FrontendTokenKind::Integer(_) => "integer-constant",
            FrontendTokenKind::Floating(_) => "floating-constant",
            FrontendTokenKind::Character(_) => "character-constant",
            FrontendTokenKind::String(_) => "string-literal",
            FrontendTokenKind::Punctuator(_) => "punctuator",
        };
        stdout.push_str(&format!(
            "{} {:?} {}:{}-{}:{}\n",
            kind, token.spelling, start.line, start.column, end.line, end.column
        ));
    }
    stdout
}

struct TemporaryObject {
    temporary: RegisteredTemporaryFile,
    file: Option<File>,
}

struct TemporaryAssembly {
    temporary: RegisteredTemporaryFile,
}

impl TemporaryAssembly {
    fn create(contents: &[u8]) -> Result<Self, DriverError> {
        for _ in 0..100 {
            let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ccc-{}-{id}.s", std::process::id()));
            match RegisteredTemporaryFile::create(path) {
                Ok((mut file, temporary)) => {
                    file.write_all(contents).map_err(|error| {
                        DriverError::new(format!(
                            "ccc: cannot write a temporary assembly input: {error}"
                        ))
                    })?;
                    file.sync_all().map_err(|error| {
                        DriverError::new(format!(
                            "ccc: cannot prepare a temporary assembly input: {error}"
                        ))
                    })?;
                    return Ok(Self { temporary });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(DriverError::new(format!(
                        "ccc: cannot create a temporary assembly input: {error}"
                    )));
                }
            }
        }
        Err(DriverError::new(
            "ccc: cannot allocate a collision-free temporary assembly path",
        ))
    }

    fn path(&self) -> &Path {
        self.temporary.path()
    }
}

impl TemporaryObject {
    fn create() -> Result<Self, DriverError> {
        for _ in 0..100 {
            let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ccc-{}-{id}.o", std::process::id()));
            match RegisteredTemporaryFile::create(path) {
                Ok((file, temporary)) => {
                    return Ok(Self {
                        temporary,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(DriverError::new(format!(
                        "ccc: cannot create a temporary object: {error}"
                    )));
                }
            }
        }
        Err(DriverError::new(
            "ccc: cannot allocate a collision-free temporary object path",
        ))
    }

    fn prepare_external_write(&mut self) -> Result<&Path, DriverError> {
        if let Some(file) = self.file.take() {
            file.sync_all().map_err(|error| {
                DriverError::new(format!("ccc: cannot prepare temporary object: {error}"))
            })?;
        }
        Ok(self.path())
    }

    fn path(&self) -> &Path {
        self.temporary.path()
    }
}

fn rendered_driver_error(rendered: String) -> DriverError {
    if rendered.trim().is_empty() {
        DriverError::new("ccc: preprocessing failed")
    } else {
        DriverError::new(format!("ccc: {}", rendered.trim_end()))
    }
}

fn successful_diagnostics(
    sources: &SourceMap,
    diagnostics: &DiagnosticEngine,
    format: DiagnosticFormat,
) -> String {
    if diagnostics.diagnostics().is_empty() {
        String::new()
    } else {
        diagnostics.render_format(sources, format)
    }
}

fn render_diagnostics_for_error(
    sources: &SourceMap,
    diagnostics: &DiagnosticEngine,
    format: DiagnosticFormat,
) -> String {
    diagnostics.render_format(sources, format)
}

fn diagnostic_engine_error(
    sources: &SourceMap,
    diagnostics: &DiagnosticEngine,
    format: DiagnosticFormat,
) -> DriverError {
    let rendered = render_diagnostics_for_error(sources, diagnostics, format);
    match format {
        DiagnosticFormat::Text => rendered_driver_error(rendered),
        DiagnosticFormat::Json => {
            DriverError::with_json_document(rendered.trim_end(), rendered.clone())
        }
    }
}

fn diagnostic_error(sources: &SourceMap, diagnostic: Diagnostic) -> DriverError {
    let message = format!("ccc: {}", diagnostic.render(sources).trim_end());
    let json = render_json_document(&[diagnostic], sources, RenderOptions::default());
    DriverError::with_json_document(message, json)
}

fn owner_error(code: &'static str, message: impl Into<String>) -> DriverError {
    diagnostic_error(&SourceMap::new(), Diagnostic::error(code, message))
}

fn with_prior_diagnostics(prior: &str, error: DriverError) -> DriverError {
    if prior.trim().is_empty() {
        error
    } else if is_json_diagnostic_document(prior) {
        let error = error_for_format(error, DiagnosticFormat::Json);
        let merged = merge_json_documents(
            prior,
            error
                .json_document
                .as_deref()
                .expect("JSON-formatted driver errors carry a document"),
        )
        .unwrap_or_else(|| format!("{prior}{}", error.message));
        DriverError::with_json_document(merged.trim_end(), merged.clone())
    } else {
        let mut combined = prior.to_owned();
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&error.to_string());
        DriverError::new(combined)
    }
}

fn error_for_format(error: DriverError, format: DiagnosticFormat) -> DriverError {
    if format == DiagnosticFormat::Text {
        return error;
    }
    if let Some(json) = &error.json_document {
        return DriverError::with_json_document(json.trim_end(), json.clone());
    }
    let message = error
        .message
        .strip_prefix("ccc: ")
        .unwrap_or(&error.message)
        .to_owned();
    let diagnostic = Diagnostic::error("CCC6000", message).with_category("driver");
    let json = render_json_document(&[diagnostic], &SourceMap::new(), RenderOptions::default());
    DriverError::with_json_document(json.trim_end(), json.clone())
}

fn prepend_json_to_error(document: &str, error: DriverError) -> DriverError {
    let error = error_for_format(error, DiagnosticFormat::Json);
    let merged = merge_json_documents(
        document,
        error
            .json_document
            .as_deref()
            .expect("JSON-formatted driver errors carry a document"),
    )
    .unwrap_or_else(|| format!("{document}{}", error.message));
    DriverError::with_json_document(merged.trim_end(), merged.clone())
}

fn is_json_diagnostic_document(document: &str) -> bool {
    document
        .trim()
        .starts_with("{\"schema_version\":1,\"diagnostics\":[")
}

fn append_diagnostic_output(accumulated: &mut String, next: &str, format: DiagnosticFormat) {
    if next.trim().is_empty() {
        return;
    }
    if format == DiagnosticFormat::Text {
        accumulated.push_str(next);
        return;
    }

    let as_document = |output: &str| {
        if is_json_diagnostic_document(output) {
            output.to_owned()
        } else {
            render_json_document(
                &[Diagnostic::warning("CCC6006", "driver", output.trim())],
                &SourceMap::new(),
                RenderOptions::default(),
            )
        }
    };
    let next = as_document(next);
    if accumulated.trim().is_empty() {
        *accumulated = next;
        return;
    }
    let current = as_document(accumulated);
    *accumulated = merge_json_documents(&current, &next)
        .expect("normalized diagnostic outputs are valid JSON documents");
}

fn merge_json_documents(first: &str, second: &str) -> Option<String> {
    const PREFIX: &str = "{\"schema_version\":1,\"diagnostics\":[";
    const SUFFIX: &str = "]}";
    let first = first.trim();
    let second = second.trim();
    let first_items = first.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let second_items = second.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let separator = (!first_items.is_empty() && !second_items.is_empty()).then_some(',');
    let mut merged = String::from(PREFIX);
    merged.push_str(first_items);
    if let Some(separator) = separator {
        merged.push(separator);
    }
    merged.push_str(second_items);
    merged.push_str("]}\n");
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn version_reports_the_driver_identity_without_an_input() {
        let output = run(["--version".to_owned()]).unwrap();
        assert_eq!(
            output.stdout,
            format!("ccc {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty());
    }

    fn parsed_driver_options(arguments: &[&str]) -> DriverOptions {
        let ParsedCommand::Run(options) =
            args::parse(arguments.iter().map(|argument| (*argument).to_owned())).unwrap()
        else {
            panic!("expected runnable driver options");
        };
        *options
    }

    #[test]
    fn degraded_hardening_uses_the_effective_warning_policy() {
        let cases: &[(&[&str], Option<&str>)] = &[
            (&["-fstack-protector-strong", "input.c"], Some("warning")),
            (
                &["-fstack-protector-strong", "-Werror", "input.c"],
                Some("error"),
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Wno-degraded-hardening",
                    "input.c",
                ],
                None,
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Wno-degraded-hardening",
                    "-Wdegraded-hardening",
                    "input.c",
                ],
                Some("warning"),
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Werror=degraded-hardening",
                    "input.c",
                ],
                Some("error"),
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Werror",
                    "-Wno-error=degraded-hardening",
                    "input.c",
                ],
                Some("warning"),
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Werror=degraded-hardening",
                    "-Wno-error=degraded-hardening",
                    "input.c",
                ],
                Some("warning"),
            ),
            (
                &[
                    "-fstack-protector-strong",
                    "-Wno-error=degraded-hardening",
                    "-Werror=degraded-hardening",
                    "input.c",
                ],
                Some("error"),
            ),
            (&["-fstack-protector-strong", "-w", "input.c"], None),
        ];

        for (arguments, expected_severity) in cases {
            let options = parsed_driver_options(arguments);
            let diagnostic = degraded_hardening_diagnostics(&options);
            match expected_severity {
                None => assert_eq!(diagnostic.unwrap(), "", "{arguments:?}"),
                Some("warning") => {
                    let diagnostic = diagnostic.unwrap();
                    assert!(
                        diagnostic.contains("ccc: warning:"),
                        "{arguments:?}: {diagnostic}"
                    );
                }
                Some("error") => {
                    let diagnostic = diagnostic.unwrap_err().to_string();
                    assert!(
                        diagnostic.contains("ccc: error:"),
                        "{arguments:?}: {diagnostic}"
                    );
                }
                Some(severity) => panic!("unexpected severity {severity}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn debug_artifact_tool_runs_before_registered_objects_are_dropped() {
        let unique = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ccc-debug-artifact-contract-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("program");
        let driver = directory.join("fixture-cc");
        let dsymutil = directory.join("fixture-dsymutil");

        let mut object = TemporaryObject::create().unwrap();
        let object_path = object.prepare_external_write().unwrap().to_path_buf();
        fs::write(&object_path, b"temporary debug object").unwrap();
        fs::write(&output, b"linked executable").unwrap();
        fs::write(
            directory.join("expected-object"),
            object_path.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        fs::write(
            &dsymutil,
            "#!/bin/sh\n\
                 expected=$(cat \"$(dirname \"$0\")/expected-object\")\n\
                 test -f \"$expected\" || exit 65\n\
                 test -f \"$1\" || exit 66\n\
                 test \"$2\" = -o || exit 67\n\
                 mkdir -p \"$3/Contents/Resources/DWARF\"\n\
                 printf 'materialized while object existed\\n' > \"$3/Contents/Resources/DWARF/program\"\n",
        )
        .unwrap();
        fs::write(
            &driver,
            "#!/bin/sh\n\
                 test \"$1\" = -print-prog-name=dsymutil || exit 64\n\
                 printf '%s\\n' \"$(dirname \"$0\")/fixture-dsymutil\"\n",
        )
        .unwrap();
        make_executable(&driver);
        make_executable(&dsymutil);

        let triple = "aarch64-apple-darwin".parse().unwrap();
        let mut config = EffectiveCompilationConfig::for_target(triple).unwrap();
        config.toolchain.compiler_driver = Some(ccc_target::ToolCommandSpec::new(&driver));
        assert_eq!(
            materialize_darwin_debug_artifact(&output, &config).unwrap(),
            ""
        );
        assert!(object_path.is_file());
        assert_eq!(
            fs::read_to_string(
                darwin_debug_artifact_path(&output).join("Contents/Resources/DWARF/program")
            )
            .unwrap(),
            "materialized while object existed\n"
        );

        drop(object);
        assert!(!object_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn debug_artifact_tool_must_be_an_absolute_selected_toolchain_path() {
        let unique = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ccc-debug-artifact-path-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let driver = directory.join("fixture-cc");
        fs::write(
            &driver,
            "#!/bin/sh\n\
             test \"$1\" = -print-prog-name=dsymutil || exit 64\n\
             printf 'dsymutil\\n'\n",
        )
        .unwrap();
        make_executable(&driver);

        let triple = "aarch64-apple-darwin".parse().unwrap();
        let mut config = EffectiveCompilationConfig::for_target(triple).unwrap();
        config.toolchain.compiler_driver = Some(ccc_target::ToolCommandSpec::new(&driver));
        let error = resolve_dsymutil(&config).unwrap_err().to_string();
        assert!(error.contains("CCC6006"), "{error}");
        assert!(error.contains("absolute dsymutil path"), "{error}");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enforces_the_ordered_hosted_header_phase_ceiling() {
        let mut config = EffectiveCompilationConfig::default();
        config.gnu_profile.as_mut().unwrap().scope = CompatibilityScope::Preprocessing;
        config
            .toolchain
            .system_includes
            .push(ccc_target::SystemIncludeEntry::new(
                "/target/include",
                SystemIncludeKind::System,
            ));
        let mut output = PreprocessOutput::default();
        output.dependencies.edges.push(ccc_pp::DependencyEdge {
            from: PathBuf::from("source.c"),
            to: PathBuf::from("/target/include/features.h"),
            spelled: "features.h".to_owned(),
            system: true,
        });

        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Compile { link: false }, &config, &output),
            Some(Path::new("/target/include/features.h"))
        );
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Preprocess, &config, &output),
            None
        );
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Dump(DumpKind::Tokens), &config, &output),
            Some(Path::new("/target/include/features.h"))
        );
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Dump(DumpKind::PpTokens), &config, &output),
            None
        );

        config.gnu_profile.as_mut().unwrap().scope = CompatibilityScope::Parsing;
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Dump(DumpKind::Tokens), &config, &output),
            None
        );
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Dump(DumpKind::Ast), &config, &output),
            None
        );
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Dump(DumpKind::TypedAst), &config, &output),
            Some(Path::new("/target/include/features.h"))
        );

        output.dependencies.edges.clear();
        output.items.push(PpItem::LineMarker(ccc_pp::LineMarker {
            line: 1,
            file: "/target/include/stdio.h".to_owned(),
            flags: vec![1, 3],
        }));
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Compile { link: false }, &config, &output),
            Some(Path::new("/target/include/stdio.h"))
        );

        config.resource_dir = Some(PathBuf::from("/compiler/resources"));
        config
            .toolchain
            .system_includes
            .push(ccc_target::SystemIncludeEntry::new(
                "/compiler/resources/include",
                SystemIncludeKind::Builtin,
            ));
        output.items.clear();
        output.items.push(PpItem::LineMarker(ccc_pp::LineMarker {
            line: 1,
            file: "/compiler/resources/include/stdbool.h".to_owned(),
            flags: vec![1, 3],
        }));
        assert_eq!(
            unsupported_hosted_header(&PrimaryAction::Compile { link: false }, &config, &output),
            None
        );
    }

    fn temporary_source(name: &str, source: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "ccc-driver-test-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join(name);
        fs::write(&input, source).unwrap();
        (directory, input)
    }

    #[test]
    fn token_dump_is_stable() {
        let (directory, input) = temporary_source("trivial.c", "int x = 42;\n");
        let output = run([
            "--dump-tokens".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap();
        assert_eq!(
            output.stdout,
            concat!(
                "keyword \"int\" 1:1-1:4\n",
                "identifier \"x\" 1:5-1:6\n",
                "punctuator \"=\" 1:7-1:8\n",
                "integer-constant \"42\" 1:9-1:11\n",
                "punctuator \";\" 1:11-1:12\n",
            )
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn renders_frontend_and_backend_dumps_after_macro_expansion() {
        let (directory, input) = temporary_source(
            "program.c",
            "#define ANSWER 42\nint main(void) { int x = ANSWER; return x; }",
        );
        let input = input.display().to_string();
        let ast = run([
            "--dump-ast".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(ast.contains("function-definition main"));
        let typed_ast = run([
            "--dump-typed-ast".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(typed_ast.contains("function @0 main : int ()"));
        assert!(typed_ast.contains("constant Signed(42) : int Value"));
        let ir = run([
            "--dump-ir".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(ir.contains("function f0 @main("));
        let abi = run([
            "--dump-abi".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(abi.contains("abi-plan schema=ccc-abi-config-v3"));
        let host_target = ccc_target::EffectiveCompilationConfig::host()
            .unwrap()
            .target
            .triple;
        assert!(abi.contains(&format!("target={host_target}")));
        assert!(abi.contains("definition function=0"));
        assert!(!abi.contains(std::env::temp_dir().to_string_lossy().as_ref()));
        let clif = run([
            "--emit=clif".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(clif.contains("function main"));
        let stats = run([
            "--emit=codegen-stats".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(stats.starts_with("schema_version\t1\n"));
        assert!(stats.contains("post_inline_ir.functions\t1\n"));
        assert!(stats.contains("primary_object.text_bytes\t"));
        assert!(stats.contains("primary_object.debug_bytes\t0\n"));
        let debug_stats = run([
            "--emit=codegen-stats".to_owned(),
            "-g".to_owned(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        let debug_bytes = debug_stats
            .lines()
            .find_map(|line| line.strip_prefix("primary_object.debug_bytes\t"))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(debug_bytes > 0);
        let dependencies = directory.join("program.d");
        let stats_with_dependencies = run([
            "--emit=codegen-stats".to_owned(),
            "-MD".to_owned(),
            "-MF".to_owned(),
            dependencies.display().to_string(),
            "-nostdinc".to_owned(),
            input.clone(),
        ])
        .unwrap()
        .stdout;
        assert!(stats_with_dependencies.lines().all(|line| {
            let Some((metric, value)) = line.split_once('\t') else {
                return false;
            };
            !metric.is_empty() && value.parse::<u64>().is_ok()
        }));
        assert!(dependencies.exists());
        let error = run([
            "--emit=codegen-stats".to_owned(),
            "-MD".to_owned(),
            "-MF".to_owned(),
            "-".to_owned(),
            "-nostdinc".to_owned(),
            input,
        ])
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires dependency output to use a file")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn compiles_a_function_object_with_a_header() {
        use object::{Object as _, ObjectSymbol as _};

        let (directory, input) = temporary_source(
            "program.c",
            "#include \"value.h\"\nint main(void) { return VALUE; }",
        );
        fs::write(directory.join("value.h"), "#define VALUE 42\n").unwrap();
        let output = directory.join("program.o");
        run([
            "-c".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
            "-o".to_owned(),
            output.display().to_string(),
        ])
        .unwrap();
        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        assert!(
            object
                .symbols()
                .any(|symbol| matches!(symbol.name(), Ok("main" | "_main")))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reports_semantic_errors_with_source_locations() {
        let (directory, input) =
            temporary_source("invalid.c", "int main(void) { return missing; }");
        let error = run([
            "-c".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("CCC2274"));
        assert!(message.contains("invalid.c:1:"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsupported_programs_do_not_emit_objects() {
        let cases = [
            (
                "atomic-i128.c",
                "_Atomic(__int128) value; int main(void) { return 0; }",
                "CCC2443",
            ),
            (
                "wide-literal.c",
                "int main(void) { return 170141183460469231731687303715884105728; }",
                "CCC2276",
            ),
            (
                "wrong-arity.c",
                "int f(int x) { return x; } int main(void) { return f(); }",
                "CCC2300",
            ),
        ];
        for (name, source, code) in cases {
            let (directory, input) = temporary_source(name, source);
            let output = directory.join("invalid.o");
            let error = run([
                "-c".to_owned(),
                "-nostdinc".to_owned(),
                input.display().to_string(),
                "-o".to_owned(),
                output.display().to_string(),
            ])
            .unwrap_err();
            assert!(error.to_string().contains(code), "{name}: {error}");
            assert!(!output.exists(), "{name} unexpectedly emitted an object");
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn reports_non_utf8_source_with_a_driver_diagnostic() {
        let (directory, input) = temporary_source("invalid.c", "");
        fs::write(&input, b"int main(void) { /* \xff */ return 0; }").unwrap();
        let error = run([
            "-c".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("CCC6002"));
        assert!(error.to_string().contains("cannot read"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preserves_preprocessing_warnings_when_semantic_analysis_fails() {
        let (directory, input) = temporary_source(
            "warning-error.c",
            "#warning retained warning\nint main(void) { return missing; }\n",
        );
        let error = run([
            "-c".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap_err()
        .to_string();
        let warning = error.find("CCC1315").expect("preprocessing warning");
        let semantic = error.find("CCC2274").expect("semantic error");
        assert!(warning < semantic, "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preserves_preprocessing_warnings_when_dependency_writing_fails() {
        let (directory, input) =
            temporary_source("warning.c", "#warning retained warning\nint value;\n");
        let error = run([
            "-E".to_owned(),
            "-MD".to_owned(),
            "-MF".to_owned(),
            directory.display().to_string(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains("CCC1315"), "{error}");
        assert!(error.contains("cannot write dependency file"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_dependency_file_leaves_preprocess_output_destination_available() {
        let (directory, input) = temporary_source("source.c", "int value;\n");
        let dependencies = directory.join("source.d");
        let preprocessed = directory.join("source.i");
        let output = run([
            "-E".to_owned(),
            "-P".to_owned(),
            "-MD".to_owned(),
            "-MF".to_owned(),
            dependencies.display().to_string(),
            "-o".to_owned(),
            preprocessed.display().to_string(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap();
        assert!(output.stdout.is_empty());
        assert!(
            fs::read_to_string(&preprocessed)
                .unwrap()
                .contains("int value;")
        );
        let dependency_text = fs::read_to_string(&dependencies).unwrap();
        assert!(dependency_text.starts_with(&format!("{}:", preprocessed.display())));
        assert!(dependency_text.contains(&input.display().to_string()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preprocess_output_path_is_the_dependency_destination_without_mf() {
        let (directory, input) = temporary_source("source.c", "int value;\n");
        let dependencies = directory.join("source.d");
        let output = run([
            "-E".to_owned(),
            "-P".to_owned(),
            "-MD".to_owned(),
            "-o".to_owned(),
            dependencies.display().to_string(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap();
        assert!(output.stdout.contains("int value;"));
        assert!(
            fs::read_to_string(&dependencies)
                .unwrap()
                .contains("source.o:")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn implicit_compile_dependency_uses_the_source_basename() {
        let unique = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("implicit-{}-{unique}.c", std::process::id());
        let (directory, input) = temporary_source(&name, "int main(void) { return 0; }\n");
        let dependency_name = Path::new(&name).with_extension("d");
        let dependency_path = std::env::current_dir().unwrap().join(&dependency_name);

        let output = run([
            "-c".to_owned(),
            "-MD".to_owned(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap();

        assert!(output.stdout.is_empty());
        let dependencies = fs::read_to_string(&dependency_path).unwrap();
        let target = Path::new(&name).with_extension("o");
        let object_path = std::env::current_dir().unwrap().join(&target);
        assert!(
            dependencies.starts_with(&format!("{}:", target.display())),
            "{dependencies}"
        );
        assert!(!dependencies.starts_with("a.out:"), "{dependencies}");

        fs::remove_file(dependency_path).unwrap();
        fs::remove_file(object_path).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dependency_dash_destination_writes_to_stdout() {
        let (directory, input) =
            temporary_source("stdout-dependency.c", "int main(void) { return 0; }\n");
        let object = directory.join("stdout-dependency.o");

        let output = run([
            "-c".to_owned(),
            "-MD".to_owned(),
            "-MF".to_owned(),
            "-".to_owned(),
            "-o".to_owned(),
            object.display().to_string(),
            "-nostdinc".to_owned(),
            input.display().to_string(),
        ])
        .unwrap();

        assert!(
            output.stdout.starts_with(&format!("{}:", object.display())),
            "{}",
            output.stdout
        );
        assert!(object.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
