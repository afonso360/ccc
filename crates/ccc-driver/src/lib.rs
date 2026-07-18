//! The `ccc` command-line driver.

mod args;
mod atomic_output;
mod dependency;
mod diagnostics;
mod empty_object;
mod predefined;
mod resource;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use args::{
    DependencyMode as DriverDependencyMode, DependencyTarget, DriverOptions, DumpKind,
    ForcedInputKind, IncludePathKind as DriverIncludePathKind, MacroAction, ParsedCommand,
    PrimaryAction,
};
use ccc_codegen::{Options as CodegenOptions, Output as CodegenOutput};
use ccc_diag::Diagnostic;
use ccc_ir::generic::{FullModule, IrError as FrontendIrError};
use ccc_link::{ToolchainRequirements, ToolchainResolver};
use ccc_pp::{
    CommandLineMacro, DependencyMode, FileProvider, FsFileProvider, IncludePath, IncludePathKind,
    PpItem, PpToken, PreprocessContext, PreprocessLimits, PreprocessOptions, PreprocessOutput,
    preprocess, render_macro_definitions, render_preprocessed,
};
use ccc_sema::generic::{FullTypedTranslationUnit, analyze_frontend, dump_frontend_typed_ast};
use ccc_session::{Session, SourceFileSpec, SourceMap};
use ccc_syntax::frontend::{
    FrontendItem, TokenKind as FrontendTokenKind, TranslationUnit as FrontendTranslationUnit,
    convert_pp_items, dump_ast as dump_frontend_ast, parse_with_mode as parse_frontend_with_mode,
};
use ccc_target::{CompatibilityScope, EffectiveCompilationConfig, SystemIncludeKind};

use dependency::{
    DependencyRecord, DependencyRenderOptions, MakeTarget, default_dependency_path,
    render_dependencies,
};
use diagnostics::PreprocessorDiagnostics;
use resource::ResourceDirectory;

pub use empty_object::is_empty_elf64_relocatable;

const HELP: &str = "Usage: ccc [options] <input.c>\n\
  -c                         Compile without linking\n\
  -E [-P]                    Preprocess only; -P suppresses linemarkers\n\
  -O|-O0|-O1|-O2|-O3|-Os|-Oz\n\
  -g|-gN                     Accepted compatibility options; currently no code-generation effect\n\
  -fPIC|-fPIE|-pie           Select position-independent code and PIE linking (default)\n\
  -fno-pic|-fno-pie|-no-pie Select static-model code and non-PIE linking\n\
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
  --version                  Print the CCC driver version\n";

const VERSION: &str = concat!("ccc ", env!("CARGO_PKG_VERSION"), "\n");

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Eq, PartialEq)]
pub struct DriverError {
    message: String,
}

impl DriverError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
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
    match args::parse(arguments).map_err(DriverError::new)? {
        ParsedCommand::Help => Ok(DriverOutput {
            stdout: HELP.to_owned(),
            stderr: String::new(),
        }),
        ParsedCommand::Version => Ok(DriverOutput {
            stdout: VERSION.to_owned(),
            stderr: String::new(),
        }),
        ParsedCommand::Run(options) => execute(*options),
    }
}

fn execute(options: DriverOptions) -> Result<DriverOutput, DriverError> {
    let prepared = preprocess_source(&options)?;

    if matches!(options.dependencies.mode, DriverDependencyMode::Only { .. }) {
        return dependency_only_output(&options, prepared);
    }

    match options.action {
        PrimaryAction::Preprocess => preprocess_output(&options, prepared),
        PrimaryAction::Dump(kind) => dump_output(&options, prepared, kind),
        PrimaryAction::Compile { link } => compile_output(&options, prepared, link),
    }
}

struct PreparedSource {
    session: Session,
    output: PreprocessOutput,
    stderr: String,
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
    let mut sink = PreprocessorDiagnostics::new(&options.warning_options);
    let output = preprocess(
        &mut PreprocessContext {
            session: &mut session,
            diagnostics: &mut sink,
            options: &pp_options,
            files: &provider,
        },
        main_file,
    );

    let warn_in_system_headers = warning_toggle(
        &options.warning_options,
        "-Wsystem-headers",
        "-Wno-system-headers",
        false,
    );
    let diagnostics = sink.finish(
        &session.sources,
        !options.suppress_warnings,
        options.warnings_as_errors,
        warn_in_system_headers,
        options.error_limit.unwrap_or(20),
    );
    let stderr = diagnostics.render(&session.sources);
    if output.had_errors || diagnostics.has_errors() {
        return Err(rendered_driver_error(stderr));
    }

    if let Some(header) = unsupported_hosted_header(&options.action, &session.config, &output) {
        let required = required_compatibility_scope(&options.action);
        let available = session
            .config
            .gnu_profile
            .as_ref()
            .map_or(CompatibilityScope::Preprocessing, |profile| profile.scope);
        return Err(with_prior_diagnostics(
            &stderr,
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
            assembler: false,
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
        PrimaryAction::Dump(DumpKind::TypedAst) => CompatibilityScope::SemanticAnalysis,
        PrimaryAction::Compile { .. }
        | PrimaryAction::Dump(DumpKind::Ir | DumpKind::Abi | DumpKind::Clif) => {
            CompatibilityScope::CodeGeneration
        }
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
    let prior_stderr = prepared.stderr.clone();
    let dependency_stdout = if matches!(
        options.dependencies.mode,
        DriverDependencyMode::SideEffect { .. }
    ) {
        write_side_effect_dependencies(options, &prepared.output, None)
            .map_err(|error| with_prior_diagnostics(&prior_stderr, error))?
    } else {
        String::new()
    };

    let stdout = match kind {
        DumpKind::PpTokens => dump_pp_tokens(&prepared.session.sources, &prepared.output.tokens()),
        DumpKind::Tokens => {
            let items = convert_pp_items(prepared.output.items).map_err(|error| {
                with_prior_diagnostics(
                    &prior_stderr,
                    diagnostic_error(
                        &prepared.session.sources,
                        Diagnostic::error(error.code, error.message)
                            .with_primary(error.span, "while converting preprocessing tokens"),
                    ),
                )
            })?;
            dump_frontend_tokens(&prepared.session.sources, &items)
        }
        DumpKind::Ast => {
            let parsed =
                parse_frontend_preprocessed(prepared.session, prepared.output, &prior_stderr)?;
            let mut stdout = dump_frontend_ast(&parsed.ast);
            stdout.push_str(&dependency_stdout);
            return Ok(DriverOutput {
                stdout,
                stderr: parsed.stderr,
            });
        }
        DumpKind::TypedAst => {
            let (parsed, typed) = analyze_frontend_preprocessed(prepared, &prior_stderr)?;
            let mut stdout = dump_frontend_typed_ast(&typed);
            stdout.push_str(&dependency_stdout);
            return Ok(DriverOutput {
                stdout,
                stderr: parsed.stderr,
            });
        }
        DumpKind::Ir => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared, &prior_stderr)?;
            let mut stdout = ccc_ir::generic::dump_frontend_ir(&ir);
            stdout.push_str(&dependency_stdout);
            return Ok(DriverOutput {
                stdout,
                stderr: parsed.stderr,
            });
        }
        DumpKind::Abi => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared, &prior_stderr)?;
            let plan = ccc_abi::plan_module(&ir, &parsed.session.config)
                .map_err(|error| abi_driver_error(&parsed.session.sources, error))?;
            let verified = plan
                .verify_against(&ir, &parsed.session.config)
                .map_err(|error| abi_driver_error(&parsed.session.sources, error))?;
            let mut stdout = ccc_abi::dump_module_plan(verified);
            stdout.push_str(&dependency_stdout);
            return Ok(DriverOutput {
                stdout,
                stderr: parsed.stderr,
            });
        }
        DumpKind::Clif => {
            let (parsed, ir) = lower_frontend_preprocessed(prepared, &prior_stderr)?;
            let generated =
                codegen_frontend(&ir, &parsed.session.config, &parsed.session.sources, true)
                    .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
            let mut stdout = generated.clif;
            stdout.push_str(&dependency_stdout);
            return Ok(DriverOutput {
                stdout,
                stderr: parsed.stderr,
            });
        }
    };
    let mut stdout = stdout;
    stdout.push_str(&dependency_stdout);
    Ok(DriverOutput {
        stdout,
        stderr: prepared.stderr,
    })
}

fn compile_output(
    options: &DriverOptions,
    prepared: PreparedSource,
    link: bool,
) -> Result<DriverOutput, DriverError> {
    let output = options.output.clone().unwrap_or_else(|| {
        if link {
            PathBuf::from("a.out")
        } else {
            options.input.with_extension("o")
        }
    });
    let dependency_stdout = if matches!(
        options.dependencies.mode,
        DriverDependencyMode::SideEffect { .. }
    ) {
        write_side_effect_dependencies(options, &prepared.output, options.output.as_deref())
            .map_err(|error| with_prior_diagnostics(&prepared.stderr, error))?
    } else {
        String::new()
    };

    let prior_stderr = prepared.stderr.clone();
    let (parsed, ir) = lower_frontend_preprocessed(prepared, &prior_stderr)?;
    let generated = codegen_frontend(&ir, &parsed.session.config, &parsed.session.sources, false)
        .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
    let artifact = generated.into_artifact_bundle();
    if link {
        let mut temporary = TemporaryObject::create()
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?;
        let artifact_path = temporary
            .prepare_external_write()
            .map_err(|error| with_prior_diagnostics(&parsed.stderr, error))?
            .to_path_buf();
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
        ccc_link::package_artifact_bundle(artifact, &output, &parsed.session.config).map_err(
            |error| with_prior_diagnostics(&parsed.stderr, owner_error(error.code, error.message)),
        )?;
    }
    Ok(DriverOutput {
        stdout: dependency_stdout,
        stderr: parsed.stderr,
    })
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

fn write_side_effect_dependencies(
    options: &DriverOptions,
    output: &PreprocessOutput,
    output_target: Option<&Path>,
) -> Result<String, DriverError> {
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
    if destination == Path::new("-") {
        return Ok(rendered.contents);
    }
    atomic_output::write_atomic(&destination, rendered.as_bytes())
        .map_err(|error| {
            DriverError::new(format!(
                "ccc: cannot write dependency file {}: {error}",
                destination.display()
            ))
        })
        .map(|()| String::new())
}

struct FrontendParsedSource {
    session: Session,
    ast: FrontendTranslationUnit,
    stderr: String,
}

fn parse_frontend_preprocessed(
    session: Session,
    output: PreprocessOutput,
    prior_stderr: &str,
) -> Result<FrontendParsedSource, DriverError> {
    let items = convert_pp_items(output.items).map_err(|error| {
        with_prior_diagnostics(
            prior_stderr,
            diagnostic_error(
                &session.sources,
                Diagnostic::error(error.code, error.message)
                    .with_primary(error.span, "while converting preprocessing tokens"),
            ),
        )
    })?;
    let ast = parse_frontend_with_mode(&items, session.config.language.mode).map_err(|error| {
        with_prior_diagnostics(
            prior_stderr,
            diagnostic_error(
                &session.sources,
                Diagnostic::error(error.code, error.message)
                    .with_primary(error.span, "while parsing"),
            ),
        )
    })?;
    Ok(FrontendParsedSource {
        session,
        ast,
        stderr: prior_stderr.to_owned(),
    })
}

fn analyze_frontend_preprocessed(
    prepared: PreparedSource,
    prior_stderr: &str,
) -> Result<(FrontendParsedSource, FullTypedTranslationUnit), DriverError> {
    let parsed = parse_frontend_preprocessed(prepared.session, prepared.output, prior_stderr)?;
    let typed = analyze_frontend(&parsed.ast, &parsed.session.config).map_err(|diagnostics| {
        with_prior_diagnostics(
            prior_stderr,
            diagnostics_error(&parsed.session.sources, diagnostics),
        )
    })?;
    Ok((parsed, typed))
}

fn lower_frontend_preprocessed(
    prepared: PreparedSource,
    prior_stderr: &str,
) -> Result<(FrontendParsedSource, FullModule), DriverError> {
    let (parsed, typed) = analyze_frontend_preprocessed(prepared, prior_stderr)?;
    let ir = ccc_ir::generic::lower_frontend(&typed).map_err(|error| {
        with_prior_diagnostics(
            prior_stderr,
            frontend_ir_error(&parsed.session.sources, error),
        )
    })?;
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
) -> Result<CodegenOutput, DriverError> {
    ccc_codegen::generic::emit(ir, config, CodegenOptions { emit_clif }).map_err(|error| {
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

fn warning_toggle(options: &[String], enable: &str, disable: &str, default: bool) -> bool {
    options.iter().fold(default, |state, option| {
        if option == enable {
            true
        } else if option == disable {
            false
        } else {
            state
        }
    })
}

struct TemporaryObject {
    path: PathBuf,
    file: Option<File>,
}

impl TemporaryObject {
    fn create() -> Result<Self, DriverError> {
        for _ in 0..100 {
            let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ccc-{}-{id}.o", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
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
        Ok(&self.path)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryObject {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn rendered_driver_error(rendered: String) -> DriverError {
    if rendered.trim().is_empty() {
        DriverError::new("ccc: preprocessing failed")
    } else {
        DriverError::new(format!("ccc: {}", rendered.trim_end()))
    }
}

fn diagnostic_error(sources: &SourceMap, diagnostic: Diagnostic) -> DriverError {
    DriverError::new(format!("ccc: {}", diagnostic.render(sources).trim_end()))
}

fn owner_error(code: &'static str, message: impl Into<String>) -> DriverError {
    diagnostic_error(&SourceMap::new(), Diagnostic::error(code, message))
}

fn diagnostics_error(sources: &SourceMap, diagnostics: Vec<Diagnostic>) -> DriverError {
    let rendered = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.render(sources))
        .collect::<Vec<_>>()
        .join("");
    DriverError::new(format!("ccc: {}", rendered.trim_end()))
}

fn with_prior_diagnostics(prior: &str, error: DriverError) -> DriverError {
    if prior.trim().is_empty() {
        error
    } else {
        let mut combined = prior.to_owned();
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&error.to_string());
        DriverError::new(combined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_reports_the_driver_identity_without_an_input() {
        let output = run(["--version".to_owned()]).unwrap();
        assert_eq!(
            output.stdout,
            format!("ccc {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty());
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
        assert!(abi.contains("abi-plan schema=ccc-abi-config-v2"));
        assert!(abi.contains(&format!("target={}", ccc_target::Triple::host())));
        assert!(abi.contains("definition function=0"));
        assert!(!abi.contains(std::env::temp_dir().to_string_lossy().as_ref()));
        let clif = run(["--emit=clif".to_owned(), "-nostdinc".to_owned(), input])
            .unwrap()
            .stdout;
        assert!(clif.contains("function main"));
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
                "atomic.c",
                "int main(void) { _Atomic int value; return value; }",
                "CCC4011",
            ),
            (
                "wide-literal.c",
                "int main(void) { return 18446744073709551616ULL; }",
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
        assert!(
            dependencies.starts_with(&format!("{}:", target.display())),
            "{dependencies}"
        );
        assert!(!dependencies.starts_with("a.out:"), "{dependencies}");

        fs::remove_file(dependency_path).unwrap();
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
