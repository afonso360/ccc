//! Target tool resolution and executable link-plan execution.

pub mod artifact;
pub mod bridge;
mod package;

pub use artifact::{
    ArtifactBundle, BridgeManifestV1, GeneratedSymbol, GeneratedSymbolOwner,
    GeneratedSymbolVisibility, VerifiedArtifactBundle,
};
pub use package::{
    PackagingReport, PackagingToolIdentity, package_artifact_bundle,
    package_artifact_bundle_with_runner,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use ccc_target::{
    Architecture, EffectiveCompilationConfig, OperatingSystem, RelocationModel,
    SystemIncludeEntry, SystemIncludeKind, ToolCommandSpec, ToolchainFingerprint, ToolchainSpec,
    Triple,
};

#[derive(Debug)]
pub struct LinkError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for LinkError {}

fn artifact_error(message: impl Into<String>) -> LinkError {
    LinkError {
        code: "CCC5010",
        message: message.into(),
    }
}

/// Components required by the selected compiler actions.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ToolchainRequirements {
    pub system_headers: bool,
    pub disable_system_headers: bool,
    pub assembler: bool,
    pub linker: bool,
    pub object_copier: bool,
    pub archiver: bool,
}

impl ToolchainRequirements {
    pub const fn preprocess(default_system_includes: bool) -> Self {
        Self {
            system_headers: default_system_includes,
            disable_system_headers: !default_system_includes,
            assembler: false,
            linker: false,
            object_copier: false,
            archiver: false,
        }
    }

    pub const fn compile(default_system_includes: bool) -> Self {
        Self {
            system_headers: default_system_includes,
            disable_system_headers: !default_system_includes,
            assembler: true,
            linker: false,
            object_copier: false,
            archiver: false,
        }
    }

    pub const fn link() -> Self {
        Self {
            system_headers: false,
            disable_system_headers: false,
            assembler: false,
            linker: true,
            object_copier: false,
            archiver: false,
        }
    }

    pub const fn archive() -> Self {
        Self {
            system_headers: false,
            disable_system_headers: false,
            assembler: false,
            linker: false,
            object_copier: false,
            archiver: true,
        }
    }

    /// Tools needed to turn generated assembly into one relocatable object.
    pub const fn package_generated_assembly() -> Self {
        Self {
            system_headers: false,
            disable_system_headers: false,
            // Assembly is intentionally driven through the compiler driver;
            // the standalone assembler is not part of this contract.
            assembler: false,
            linker: false,
            object_copier: true,
            archiver: false,
        }
    }

    /// Mach-O bridge assembly uses private-external symbols and therefore
    /// needs no GNU-style post-link symbol localization tool.
    pub const fn package_generated_macho_assembly() -> Self {
        Self {
            system_headers: false,
            disable_system_headers: false,
            assembler: false,
            linker: false,
            object_copier: false,
            archiver: false,
        }
    }
}

/// A command invocation used by an injectable toolchain probe runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeRequest {
    pub command: ToolCommandSpec,
    pub arguments: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
}

/// Captured output from a toolchain probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeOutput {
    pub success: bool,
    pub status: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// An injectable boundary for deterministic resolver tests.
pub trait ProbeRunner {
    fn run(&self, request: &ProbeRequest) -> io::Result<ProbeOutput>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessProbeRunner;

impl ProbeRunner for ProcessProbeRunner {
    fn run(&self, request: &ProbeRequest) -> io::Result<ProbeOutput> {
        let mut command = Command::new(&request.command.program);
        command
            .args(&request.command.arguments)
            .args(&request.arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if request.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        let mut child = command.spawn()?;
        if let Some(input) = &request.stdin {
            child
                .stdin
                .take()
                .expect("piped probe stdin must be available")
                .write_all(input)?;
        }
        let output = child.wait_with_output()?;
        Ok(ProbeOutput {
            success: output.status.success(),
            status: output.status.to_string(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

const RELEVANT_ENVIRONMENT_VARIABLES: &[&str] = &[
    "CCC_CC",
    "CCC_OBJCOPY",
    "CC",
    "OBJCOPY",
    "PATH",
    "SDKROOT",
    "DEVELOPER_DIR",
    "TOOLCHAINS",
    "CLANG_CONFIG_FILE_SYSTEM_DIR",
    "CLANG_CONFIG_FILE_USER_DIR",
    "GCC_EXEC_PREFIX",
    "COMPILER_PATH",
    "LIBRARY_PATH",
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "OBJC_INCLUDE_PATH",
    "DEPENDENCIES_OUTPUT",
    "SUNPRO_DEPENDENCIES",
    "LC_ALL",
    "LC_MESSAGES",
    "LANG",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "TVOS_DEPLOYMENT_TARGET",
    "WATCHOS_DEPLOYMENT_TARGET",
    "XROS_DEPLOYMENT_TARGET",
    "DRIVERKIT_DEPLOYMENT_TARGET",
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EnvironmentEntry {
    name: &'static str,
    value: Option<OsString>,
}

fn relevant_environment() -> Vec<EnvironmentEntry> {
    relevant_environment_with(|name| env::var_os(name))
}

fn relevant_environment_with(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Vec<EnvironmentEntry> {
    RELEVANT_ENVIRONMENT_VARIABLES
        .iter()
        .copied()
        .map(|name| EnvironmentEntry {
            name,
            value: lookup(name),
        })
        .collect()
}

fn environment_value<'a>(environment: &'a [EnvironmentEntry], name: &str) -> Option<&'a OsStr> {
    environment
        .iter()
        .find(|entry| entry.name == name)
        .and_then(|entry| entry.value.as_deref())
}

fn current_working_directory() -> Option<PathBuf> {
    env::current_dir()
        .ok()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FileTimestamp {
    before_epoch: bool,
    seconds: u64,
    nanoseconds: u32,
}

impl FileTimestamp {
    fn from_system_time(time: SystemTime) -> Self {
        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self {
                before_epoch: false,
                seconds: duration.as_secs(),
                nanoseconds: duration.subsec_nanos(),
            },
            Err(error) => {
                let duration = error.duration();
                Self {
                    before_epoch: true,
                    seconds: duration.as_secs(),
                    nanoseconds: duration.subsec_nanos(),
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ExecutableIdentity {
    path: PathBuf,
    length: Option<u64>,
    modified: Option<FileTimestamp>,
    platform_metadata: Vec<i128>,
}

impl ExecutableIdentity {
    fn resolve(program: &Path, environment: &[EnvironmentEntry]) -> Self {
        let path = resolve_executable_path(program, environment_value(environment, "PATH"));
        let metadata = fs::metadata(&path).ok();
        Self {
            path,
            length: metadata.as_ref().map(fs::Metadata::len),
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .map(FileTimestamp::from_system_time),
            platform_metadata: metadata
                .as_ref()
                .map_or_else(Vec::new, platform_executable_metadata),
        }
    }
}

#[cfg(unix)]
fn platform_executable_metadata(metadata: &fs::Metadata) -> Vec<i128> {
    use std::os::unix::fs::MetadataExt;

    vec![
        i128::from(metadata.dev()),
        i128::from(metadata.ino()),
        i128::from(metadata.mode()),
        i128::from(metadata.mtime()),
        i128::from(metadata.mtime_nsec()),
        i128::from(metadata.ctime()),
        i128::from(metadata.ctime_nsec()),
    ]
}

#[cfg(not(unix))]
fn platform_executable_metadata(_metadata: &fs::Metadata) -> Vec<i128> {
    Vec::new()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CommandCacheKey {
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl From<&ToolCommandSpec> for CommandCacheKey {
    fn from(command: &ToolCommandSpec) -> Self {
        Self {
            program: command.program.clone(),
            arguments: command.arguments.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ExistingToolchainCacheKey {
    compiler_driver: Option<CommandCacheKey>,
    assembler: Option<CommandCacheKey>,
    linker_driver: Option<CommandCacheKey>,
    object_copier: Option<CommandCacheKey>,
    archiver: Option<CommandCacheKey>,
    ranlib: Option<CommandCacheKey>,
    sysroot: Option<PathBuf>,
    resource_dir: Option<PathBuf>,
    system_includes: Vec<(PathBuf, SystemIncludeKind)>,
    fingerprint_digest: Option<String>,
}

impl From<&ToolchainSpec> for ExistingToolchainCacheKey {
    fn from(spec: &ToolchainSpec) -> Self {
        Self {
            compiler_driver: spec.compiler_driver.as_ref().map(Into::into),
            assembler: spec.assembler.as_ref().map(Into::into),
            linker_driver: spec.linker_driver.as_ref().map(Into::into),
            object_copier: spec.object_copier.as_ref().map(Into::into),
            archiver: spec.archiver.as_ref().map(Into::into),
            ranlib: spec.ranlib.as_ref().map(Into::into),
            sysroot: spec.sysroot.clone(),
            resource_dir: spec.resource_dir.clone(),
            system_includes: spec
                .system_includes
                .iter()
                .map(|entry| (entry.path.clone(), entry.kind))
                .collect(),
            fingerprint_digest: spec
                .fingerprint
                .as_ref()
                .map(|fingerprint| fingerprint.digest.clone()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ResolutionCacheKey {
    executable: ExecutableIdentity,
    driver_version: String,
    driver_program: PathBuf,
    driver_arguments: Vec<OsString>,
    target: String,
    target_arguments: Vec<OsString>,
    explicit_sysroot: Option<PathBuf>,
    explicit_resource_dir: Option<PathBuf>,
    requirements: ToolchainRequirements,
    environment: Vec<EnvironmentEntry>,
    working_directory: Option<PathBuf>,
    existing: ExistingToolchainCacheKey,
}

struct ResolutionInputs<'a> {
    target: &'a Triple,
    existing: &'a ToolchainSpec,
    candidate: &'a ToolCommandSpec,
    target_arguments: &'a [OsString],
    explicit_sysroot: Option<&'a Path>,
    explicit_resource_dir: Option<&'a Path>,
    requirements: ToolchainRequirements,
    executable: &'a ExecutableIdentity,
    driver_version: &'a str,
    environment: &'a [EnvironmentEntry],
    working_directory: Option<&'a Path>,
}

fn resolution_cache_key(inputs: &ResolutionInputs<'_>) -> ResolutionCacheKey {
    ResolutionCacheKey {
        executable: inputs.executable.clone(),
        driver_version: inputs.driver_version.to_owned(),
        driver_program: inputs.candidate.program.clone(),
        driver_arguments: inputs.candidate.arguments.clone(),
        target: inputs.target.to_string(),
        target_arguments: inputs.target_arguments.to_vec(),
        explicit_sysroot: inputs.explicit_sysroot.map(Path::to_path_buf),
        explicit_resource_dir: inputs.explicit_resource_dir.map(Path::to_path_buf),
        requirements: inputs.requirements,
        environment: inputs.environment.to_vec(),
        working_directory: inputs.working_directory.map(Path::to_path_buf),
        existing: inputs.existing.into(),
    }
}

const RESOLUTION_CACHE_CAPACITY: usize = 64;

#[derive(Debug)]
struct ResolutionCache {
    entries: HashMap<ResolutionCacheKey, ToolchainSpec>,
    insertion_order: VecDeque<ResolutionCacheKey>,
    capacity: usize,
}

impl ResolutionCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&self, key: &ResolutionCacheKey) -> Option<ToolchainSpec> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: ResolutionCacheKey, spec: ToolchainSpec) {
        if self.capacity == 0 {
            return;
        }
        if let Some(existing) = self.entries.get_mut(&key) {
            *existing = spec;
            return;
        }
        while self.entries.len() >= self.capacity {
            let Some(oldest) = self.insertion_order.pop_front() else {
                self.entries.clear();
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(key.clone());
        self.entries.insert(key, spec);
    }
}

fn process_resolution_cache() -> &'static Mutex<ResolutionCache> {
    static CACHE: OnceLock<Mutex<ResolutionCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ResolutionCache::new(RESOLUTION_CACHE_CAPACITY)))
}

fn resolve_with_cache(
    cache: &Mutex<ResolutionCache>,
    key: ResolutionCacheKey,
    resolve: impl FnOnce() -> Result<ToolchainSpec, LinkError>,
    inputs_still_match: impl FnOnce() -> bool,
) -> Result<ToolchainSpec, LinkError> {
    if let Some(spec) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return Ok(spec);
    }

    let spec = resolve()?;
    if inputs_still_match() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, spec.clone());
    }
    Ok(spec)
}

/// Resolves only the target-tool components required by the selected actions.
#[derive(Debug)]
pub struct ToolchainResolver<R = ProcessProbeRunner> {
    target: Triple,
    existing: ToolchainSpec,
    driver: Option<ToolCommandSpec>,
    target_arguments: Vec<OsString>,
    explicit_sysroot: Option<PathBuf>,
    explicit_resource_dir: Option<PathBuf>,
    cache_process_resolution: bool,
    runner: R,
}

impl ToolchainResolver<ProcessProbeRunner> {
    pub fn new(config: &EffectiveCompilationConfig) -> Self {
        let mut resolver = Self::with_runner(config, ProcessProbeRunner);
        resolver.cache_process_resolution = true;
        resolver
    }
}

impl<R: ProbeRunner> ToolchainResolver<R> {
    pub fn with_runner(config: &EffectiveCompilationConfig, runner: R) -> Self {
        let mut target_arguments = vec![OsString::from(format!(
            "-march={}",
            config.normalized_target_arch()
        ))];
        if matches!(
            config.target.abi,
            ccc_target::AbiIdentity::Aapcs64Lp64 | ccc_target::AbiIdentity::RiscvLp64d
        ) {
            target_arguments.push(OsString::from(format!(
                "-mabi={}",
                config.normalized_target_abi()
            )));
        }
        if let Some(version) = config.normalized_deployment_target() {
            target_arguments.push(OsString::from(format!(
                "-mmacosx-version-min={version}"
            )));
        }
        Self {
            target: config.target.triple.clone(),
            driver: config
                .toolchain
                .compiler_driver
                .clone()
                .or_else(|| config.toolchain.linker_driver.clone()),
            explicit_sysroot: config.toolchain.sysroot.clone(),
            explicit_resource_dir: config.toolchain.resource_dir.clone(),
            existing: config.toolchain.clone(),
            target_arguments,
            cache_process_resolution: false,
            runner,
        }
    }

    pub fn driver(mut self, driver: ToolCommandSpec) -> Self {
        self.driver = Some(driver);
        self
    }

    pub fn target_argument(mut self, argument: impl Into<OsString>) -> Self {
        self.target_arguments.push(argument.into());
        self
    }

    pub fn target_arguments<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.target_arguments
            .extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn sysroot(mut self, sysroot: impl Into<PathBuf>) -> Self {
        self.explicit_sysroot = Some(sysroot.into());
        self
    }

    pub fn resource_dir(mut self, resource_dir: impl Into<PathBuf>) -> Self {
        self.explicit_resource_dir = Some(resource_dir.into());
        self
    }

    pub fn resolve(&self, requirements: ToolchainRequirements) -> Result<ToolchainSpec, LinkError> {
        if self.target_arguments.is_empty()
            && requirements_satisfied(&self.existing, &self.target, requirements)
        {
            return Ok(self.existing.clone());
        }

        let environment = relevant_environment();
        let candidate = self
            .driver
            .clone()
            .map_or_else(|| driver_from_environment(&environment, &self.target), Ok)?;
        let executable = ExecutableIdentity::resolve(&candidate.program, &environment);
        let working_directory = current_working_directory();
        if self.cache_process_resolution {
            let driver_version = self.driver_version(&candidate)?;
            let key = resolution_cache_key(&ResolutionInputs {
                target: &self.target,
                existing: &self.existing,
                candidate: &candidate,
                target_arguments: &self.target_arguments,
                explicit_sysroot: self.explicit_sysroot.as_deref(),
                explicit_resource_dir: self.explicit_resource_dir.as_deref(),
                requirements,
                executable: &executable,
                driver_version: &driver_version,
                environment: &environment,
                working_directory: working_directory.as_deref(),
            });
            let validation_key = key.clone();
            return resolve_with_cache(
                process_resolution_cache(),
                key,
                || {
                    self.resolve_uncached(
                        requirements,
                        &candidate,
                        &executable,
                        &environment,
                        working_directory.as_deref(),
                        Some(driver_version.clone()),
                    )
                },
                || {
                    let fresh_environment = relevant_environment();
                    let Ok(fresh_candidate) = self
                        .driver
                        .clone()
                        .map_or_else(
                            || driver_from_environment(&fresh_environment, &self.target),
                            Ok,
                        )
                    else {
                        return false;
                    };
                    let fresh_executable =
                        ExecutableIdentity::resolve(&fresh_candidate.program, &fresh_environment);
                    let Ok(fresh_driver_version) = self.driver_version(&fresh_candidate) else {
                        return false;
                    };
                    let fresh_working_directory = current_working_directory();
                    resolution_cache_key(&ResolutionInputs {
                        target: &self.target,
                        existing: &self.existing,
                        candidate: &fresh_candidate,
                        target_arguments: &self.target_arguments,
                        explicit_sysroot: self.explicit_sysroot.as_deref(),
                        explicit_resource_dir: self.explicit_resource_dir.as_deref(),
                        requirements,
                        executable: &fresh_executable,
                        driver_version: &fresh_driver_version,
                        environment: &fresh_environment,
                        working_directory: fresh_working_directory.as_deref(),
                    }) == validation_key
                },
            );
        }
        self.resolve_uncached(
            requirements,
            &candidate,
            &executable,
            &environment,
            working_directory.as_deref(),
            None,
        )
    }

    fn resolve_uncached(
        &self,
        requirements: ToolchainRequirements,
        candidate: &ToolCommandSpec,
        executable: &ExecutableIdentity,
        environment: &[EnvironmentEntry],
        working_directory: Option<&Path>,
        preprobed_version: Option<String>,
    ) -> Result<ToolchainSpec, LinkError> {
        let reported = self.reported_target(candidate)?;
        if !target_matches(&reported, &self.target) {
            return Err(LinkError {
                code: "CCC5005",
                message: format!(
                    "compiler driver `{}` reports target `{reported}`, expected `{}`",
                    candidate.display(),
                    self.target
                ),
            });
        }

        let version = preprobed_version.map_or_else(|| self.driver_version(candidate), Ok)?;

        let needs_sysroot = requirements.system_headers || requirements.linker;
        let sysroot = if needs_sysroot {
            match &self.explicit_sysroot {
                Some(sysroot) => Some(sysroot.clone()),
                None => self.probe_sysroot(candidate, &version)?,
            }
        } else {
            self.explicit_sysroot.clone()
        };

        let (resource_dir, mut system_includes) = if requirements.system_headers {
            let resource_dir = match &self.explicit_resource_dir {
                Some(resource_dir) => Some(resource_dir.clone()),
                None => self.probe_resource_dir(candidate, sysroot.as_deref()),
            };
            let mut includes = self.probe_system_includes(candidate, sysroot.as_deref())?;
            if let Some(resource_dir) = &resource_dir {
                let resource_include_dir = resource_dir.join("include");
                for entry in &mut includes {
                    if same_path(&entry.path, resource_dir)
                        || same_path(&entry.path, &resource_include_dir)
                    {
                        entry.kind = SystemIncludeKind::Builtin;
                    }
                }
            }
            (resource_dir, includes)
        } else {
            (
                self.explicit_resource_dir.clone(),
                self.existing.system_includes.clone(),
            )
        };

        if requirements.disable_system_headers {
            system_includes.clear();
        }

        let resolved_driver = command_with_target_options(
            candidate,
            &self.target_arguments,
            self.explicit_sysroot.as_deref(),
        );
        let assembler = if requirements.assembler {
            Some(self.probe_program(candidate, "as")?)
        } else {
            self.existing.assembler.clone()
        };
        let (archiver, ranlib) = if requirements.archiver {
            (
                Some(self.probe_program(candidate, "ar")?),
                Some(self.probe_program(candidate, "ranlib")?),
            )
        } else {
            (self.existing.archiver.clone(), self.existing.ranlib.clone())
        };
        let object_copier = if requirements.object_copier {
            let explicit = environment_value(environment, "CCC_OBJCOPY")
                .or_else(|| environment_value(environment, "OBJCOPY"));
            Some(match explicit {
                Some(command) => parse_tool_command(command, "object copier")?,
                None => self.probe_program(candidate, "objcopy")?,
            })
        } else {
            self.existing.object_copier.clone()
        };

        let driver_path = executable.path.clone();
        let fingerprint_arguments = candidate
            .arguments
            .iter()
            .chain(&self.target_arguments)
            .cloned()
            .collect::<Vec<_>>();
        let digest = fingerprint_digest(&FingerprintInputs {
            executable,
            driver_program: &candidate.program,
            version: &version,
            reported_target: &reported,
            target_arguments: &fingerprint_arguments,
            sysroot: sysroot.as_deref(),
            resource_dir: resource_dir.as_deref(),
            system_includes: &system_includes,
            requirements,
            environment,
            working_directory,
        });
        let fingerprint = ToolchainFingerprint {
            driver_path,
            driver_version: version,
            reported_target: reported,
            target_arguments: fingerprint_arguments,
            sysroot: sysroot.clone(),
            resource_dir: resource_dir.clone(),
            system_includes: system_includes.clone(),
            digest,
        };

        Ok(ToolchainSpec {
            compiler_driver: Some(resolved_driver.clone()),
            assembler,
            linker_driver: requirements.linker.then_some(resolved_driver),
            object_copier,
            archiver,
            ranlib,
            sysroot,
            resource_dir,
            system_includes,
            fingerprint: Some(fingerprint),
        })
    }

    fn reported_target(&self, driver: &ToolCommandSpec) -> Result<Triple, LinkError> {
        let target = self.probe_text(
            driver,
            self.target_arguments
                .iter()
                .cloned()
                .chain([OsString::from("-dumpmachine")]),
            None,
            "target",
        )?;
        let target = target.trim();
        if target.is_empty() {
            return Err(LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` returned an empty target",
                    driver.display()
                ),
            });
        }
        target.parse().map_err(|error| LinkError {
            code: "CCC5006",
            message: format!(
                "compiler driver `{}` returned invalid target `{target}`: {error}",
                driver.display()
            ),
        })
    }

    fn driver_version(&self, driver: &ToolCommandSpec) -> Result<String, LinkError> {
        let version = self
            .probe_text(driver, [OsString::from("--version")], None, "version")?
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if version.is_empty() {
            Err(LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` returned an empty version",
                    driver.display()
                ),
            })
        } else {
            Ok(version)
        }
    }

    fn probe_sysroot(
        &self,
        driver: &ToolCommandSpec,
        driver_version: &str,
    ) -> Result<Option<PathBuf>, LinkError> {
        if driver_version.to_ascii_lowercase().contains("clang") {
            return self.probe_clang_sysroot(driver);
        }

        let output = self.probe_text(
            driver,
            self.target_arguments
                .iter()
                .cloned()
                .chain([OsString::from("--print-sysroot")]),
            None,
            "sysroot",
        )?;
        let path = output.trim();
        Ok((!path.is_empty()).then(|| PathBuf::from(path)))
    }

    fn probe_clang_sysroot(&self, driver: &ToolCommandSpec) -> Result<Option<PathBuf>, LinkError> {
        let output = self.probe(
            driver,
            self.target_arguments.iter().cloned().chain([
                OsString::from("-###"),
                OsString::from("-E"),
                OsString::from("-x"),
                OsString::from("c"),
                OsString::from("-"),
            ]),
            Some(Vec::new()),
            "effective sysroot",
        )?;
        let mut trace = String::from_utf8(output.stderr).map_err(|error| LinkError {
            code: "CCC5006",
            message: format!(
                "compiler driver `{}` returned non-UTF-8 effective sysroot trace: {error}",
                driver.display()
            ),
        })?;
        trace.push('\n');
        trace.push_str(
            &String::from_utf8(output.stdout).map_err(|error| LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` returned non-UTF-8 effective sysroot trace: {error}",
                    driver.display()
                ),
            })?,
        );

        match parse_clang_sysroot_trace(&trace) {
            Some(ClangSysrootTrace::Default) => Ok(None),
            Some(ClangSysrootTrace::Path(path)) => Ok(Some(path)),
            None => Err(LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` returned no recognizable frontend command while probing the effective sysroot",
                    driver.display()
                ),
            }),
        }
    }

    fn probe_resource_dir(
        &self,
        driver: &ToolCommandSpec,
        sysroot: Option<&Path>,
    ) -> Option<PathBuf> {
        let base_arguments = self.probe_arguments(sysroot);
        let clang_arguments = base_arguments
            .iter()
            .cloned()
            .chain([OsString::from("-print-resource-dir")]);
        if let Some(path) = self.probe_optional_path(driver, clang_arguments, "resource directory")
        {
            return Some(path);
        }

        let gcc_arguments = base_arguments
            .into_iter()
            .chain([OsString::from("-print-file-name=include")]);
        self.probe_optional_path(driver, gcc_arguments, "builtin include directory")
            .filter(|path| path != Path::new("include"))
    }

    fn probe_system_includes(
        &self,
        driver: &ToolCommandSpec,
        sysroot: Option<&Path>,
    ) -> Result<Vec<SystemIncludeEntry>, LinkError> {
        let arguments = self.probe_arguments(sysroot).into_iter().chain([
            OsString::from("-E"),
            OsString::from("-x"),
            OsString::from("c"),
            OsString::from("-v"),
            OsString::from("-"),
        ]);
        let output = self.probe(driver, arguments, Some(Vec::new()), "include search")?;
        let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stdout));
        let includes = parse_system_include_search(&text, sysroot);
        if includes.is_empty() {
            return Err(LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` returned no system include search paths",
                    driver.display()
                ),
            });
        }
        Ok(includes)
    }

    fn probe_program(
        &self,
        driver: &ToolCommandSpec,
        program: &'static str,
    ) -> Result<ToolCommandSpec, LinkError> {
        let option = format!("-print-prog-name={program}");
        let output = self.probe_text(
            driver,
            self.target_arguments
                .iter()
                .cloned()
                .chain([OsString::from(option)]),
            None,
            program,
        )?;
        let output = output.trim();
        if output.is_empty() {
            return Err(LinkError {
                code: "CCC5007",
                message: format!(
                    "compiler driver `{}` did not identify target {program}",
                    driver.display()
                ),
            });
        }
        Ok(ToolCommandSpec::new(output))
    }

    fn probe_arguments(&self, sysroot: Option<&Path>) -> Vec<OsString> {
        let mut arguments = self.target_arguments.clone();
        if let Some(sysroot) = self.explicit_sysroot.as_deref().or(sysroot) {
            arguments.push(sysroot_argument(sysroot));
        }
        arguments
    }

    fn probe_optional_path<I, S>(
        &self,
        driver: &ToolCommandSpec,
        arguments: I,
        description: &'static str,
    ) -> Option<PathBuf>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let output = self.probe(driver, arguments, None, description).ok()?;
        let path = String::from_utf8(output.stdout).ok()?;
        let path = path.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    fn probe_text<I, S>(
        &self,
        driver: &ToolCommandSpec,
        arguments: I,
        stdin: Option<Vec<u8>>,
        description: &'static str,
    ) -> Result<String, LinkError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let output = self.probe(driver, arguments, stdin, description)?;
        String::from_utf8(output.stdout).map_err(|error| LinkError {
            code: "CCC5006",
            message: format!(
                "compiler driver `{}` returned non-UTF-8 {description}: {error}",
                driver.display()
            ),
        })
    }

    fn probe<I, S>(
        &self,
        driver: &ToolCommandSpec,
        arguments: I,
        stdin: Option<Vec<u8>>,
        description: &'static str,
    ) -> Result<ProbeOutput, LinkError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let request = ProbeRequest {
            command: driver.clone(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            stdin,
        };
        let output = self.runner.run(&request).map_err(|error| LinkError {
            code: "CCC5003",
            message: format!(
                "cannot invoke target compiler driver `{}` for {description}: {error}",
                driver.display()
            ),
        })?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LinkError {
                code: "CCC5006",
                message: format!(
                    "compiler driver `{}` {description} probe failed with {}: {}",
                    driver.display(),
                    output.status,
                    stderr.trim()
                ),
            });
        }
        Ok(output)
    }
}

/// Link with a previously resolved toolchain without executing discovery probes.
pub fn link_executable_with_toolchain(
    object: &Path,
    output: &Path,
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
) -> Result<(), LinkError> {
    let driver = toolchain
        .linker_driver
        .as_ref()
        .or(toolchain.compiler_driver.as_ref())
        .ok_or_else(|| LinkError {
            code: "CCC5007",
            message: format!(
                "resolved toolchain for target `{}` has no linker driver",
                config.target.triple
            ),
        })?;
    let mut command = tool_command(driver);
    command.arg(object).arg("-o").arg(output);
    match config.relocation_model {
        RelocationModel::Static => {
            command.arg("-no-pie");
        }
        RelocationModel::Pic | RelocationModel::Pie => {
            command.arg("-pie");
        }
    }
    let result = command.output().map_err(|error| LinkError {
        code: "CCC5003",
        message: format!(
            "cannot invoke target compiler driver `{}`: {error}",
            driver.display()
        ),
    })?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(LinkError {
            code: "CCC5004",
            message: format!(
                "target compiler driver `{}` failed: {}",
                driver.display(),
                stderr.trim()
            ),
        });
    }
    Ok(())
}

/// Compatibility entry point. Configurations carrying a resolved toolchain do
/// not trigger any discovery probes.
pub fn link_executable(
    object: &Path,
    output: &Path,
    config: &EffectiveCompilationConfig,
) -> Result<(), LinkError> {
    let resolved_for_target = config
        .toolchain
        .fingerprint
        .as_ref()
        .is_some_and(|fingerprint| {
            target_matches(&fingerprint.reported_target, &config.target.triple)
        });
    if resolved_for_target
        && (config.toolchain.linker_driver.is_some() || config.toolchain.compiler_driver.is_some())
    {
        return link_executable_with_toolchain(object, output, config, &config.toolchain);
    }
    let toolchain = ToolchainResolver::new(config).resolve(ToolchainRequirements::link())?;
    link_executable_with_toolchain(object, output, config, &toolchain)
}

/// Parse the stable GCC/Clang verbose preprocessor include-search section.
pub fn parse_system_include_search(
    output: &str,
    sysroot: Option<&Path>,
) -> Vec<SystemIncludeEntry> {
    #[derive(Clone, Copy)]
    enum SearchSection {
        Quote,
        System,
    }

    let mut section = None;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        match trimmed {
            "#include \"...\" search starts here:" => {
                section = Some(SearchSection::Quote);
                continue;
            }
            "#include <...> search starts here:" => {
                section = Some(SearchSection::System);
                continue;
            }
            "End of search list." => {
                section = None;
                continue;
            }
            _ => {}
        }

        let Some(section) = section else {
            continue;
        };
        if !line.starts_with(char::is_whitespace)
            || trimmed.is_empty()
            || trimmed.starts_with("ignoring ")
        {
            continue;
        }

        let (path, framework) = trimmed
            .strip_suffix(" (framework directory)")
            .map_or((trimmed, false), |path| (path, true));
        let path = resolve_sysroot_path(path, sysroot);
        if !seen.insert(path.clone()) {
            continue;
        }
        let kind = if framework {
            SystemIncludeKind::Framework
        } else {
            match section {
                SearchSection::Quote => SystemIncludeKind::Quote,
                SearchSection::System => SystemIncludeKind::System,
            }
        };
        entries.push(SystemIncludeEntry::new(path, kind));
    }
    entries
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClangSysrootTrace {
    Default,
    Path(PathBuf),
}

fn parse_clang_sysroot_trace(output: &str) -> Option<ClangSysrootTrace> {
    let mut saw_frontend_command = false;
    let mut effective_sysroot = None;
    for line in output.lines() {
        if !line.contains("\"-cc1\"") {
            continue;
        }
        let arguments = parse_clang_trace_arguments(line)?;
        if !arguments.iter().any(|argument| argument == "-cc1") {
            continue;
        }
        saw_frontend_command = true;

        let mut index = 0;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "-isysroot" | "--sysroot" => {
                    index += 1;
                    let path = arguments.get(index)?;
                    effective_sysroot = Some(PathBuf::from(path));
                }
                argument => {
                    if let Some(path) = argument
                        .strip_prefix("-isysroot=")
                        .or_else(|| argument.strip_prefix("--sysroot="))
                    {
                        effective_sysroot = Some(PathBuf::from(path));
                    }
                }
            }
            index += 1;
        }
    }

    saw_frontend_command
        .then(|| effective_sysroot.map_or(ClangSysrootTrace::Default, ClangSysrootTrace::Path))
}

fn parse_clang_trace_arguments(line: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character != '"' {
            continue;
        }

        let mut argument = String::new();
        loop {
            match characters.next()? {
                '"' => break,
                '\\' => argument.push(characters.next()?),
                character => argument.push(character),
            }
        }
        arguments.push(argument);
    }
    Some(arguments)
}

pub fn target_matches(reported: &Triple, expected: &Triple) -> bool {
    let is_macos = |operating_system: OperatingSystem| {
        matches!(
            operating_system,
            OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_)
        )
    };
    let operating_system_matches = reported.operating_system == expected.operating_system
        || (is_macos(reported.operating_system) && is_macos(expected.operating_system));
    reported.architecture == expected.architecture
        && operating_system_matches
        && reported.environment == expected.environment
        && reported.binary_format == expected.binary_format
}

fn requirements_satisfied(
    spec: &ToolchainSpec,
    target: &Triple,
    requirements: ToolchainRequirements,
) -> bool {
    let Some(fingerprint) = &spec.fingerprint else {
        return false;
    };
    target_matches(&fingerprint.reported_target, target)
        && spec.compiler_driver.is_some()
        && (!requirements.disable_system_headers || spec.system_includes.is_empty())
        && (!requirements.system_headers || !spec.system_includes.is_empty())
        && (!requirements.assembler || spec.assembler.is_some())
        && (!requirements.linker || spec.linker_driver.is_some())
        && (!requirements.object_copier || spec.object_copier.is_some())
        && (!requirements.archiver || (spec.archiver.is_some() && spec.ranlib.is_some()))
}

fn driver_from_environment(
    environment: &[EnvironmentEntry],
    target: &Triple,
) -> Result<ToolCommandSpec, LinkError> {
    environment_value(environment, "CCC_CC")
        .or_else(|| environment_value(environment, "CC"))
        .map(OsStr::to_os_string)
        .map_or_else(
            || {
                Ok(match target.architecture {
                    Architecture::Aarch64(_) if target.operating_system == OperatingSystem::Linux => {
                        ToolCommandSpec::new("aarch64-linux-gnu-gcc")
                    }
                    Architecture::Riscv64(_) => ToolCommandSpec::new("riscv64-linux-gnu-gcc"),
                    Architecture::Aarch64(_)
                        if matches!(
                            target.operating_system,
                            OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_)
                        ) =>
                    {
                        ToolCommandSpec::with_arguments("xcrun", [OsString::from("clang")])
                    }
                    _ => ToolCommandSpec::new("cc"),
                })
            },
            parse_driver_command,
        )
}

fn parse_driver_command(value: OsString) -> Result<ToolCommandSpec, LinkError> {
    parse_tool_command(&value, "target compiler driver")
}

fn parse_tool_command(value: &OsStr, description: &str) -> Result<ToolCommandSpec, LinkError> {
    let value = value.to_string_lossy();
    let mut words = value.split_whitespace();
    let program = words.next().ok_or_else(|| LinkError {
        code: "CCC5002",
        message: format!("{description} environment entry is empty"),
    })?;
    Ok(ToolCommandSpec::with_arguments(
        program,
        words.map(OsString::from),
    ))
}

fn command_with_target_options(
    driver: &ToolCommandSpec,
    target_arguments: &[OsString],
    explicit_sysroot: Option<&Path>,
) -> ToolCommandSpec {
    let mut command = driver.clone();
    command.arguments.extend_from_slice(target_arguments);
    if let Some(sysroot) = explicit_sysroot {
        command.arguments.push(sysroot_argument(sysroot));
    }
    command
}

fn sysroot_argument(sysroot: &Path) -> OsString {
    let mut argument = OsString::from("--sysroot=");
    argument.push(sysroot);
    argument
}

fn resolve_sysroot_path(path: &str, sysroot: Option<&Path>) -> PathBuf {
    let suffix = path
        .strip_prefix("$SYSROOT")
        .or_else(|| path.strip_prefix('='));
    match (suffix, sysroot) {
        (Some(suffix), Some(sysroot)) => sysroot.join(suffix.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .and_then(|left| fs::canonicalize(right).map(|right| left == right))
            .unwrap_or(false)
}

fn tool_command(spec: &ToolCommandSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command.args(&spec.arguments);
    command
}

fn resolve_executable_path(program: &Path, path: Option<&OsStr>) -> PathBuf {
    if program.components().count() > 1 {
        return fs::canonicalize(program).unwrap_or_else(|_| program.to_owned());
    }
    let Some(path) = path else {
        return program.to_owned();
    };
    env::split_paths(path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(&candidate).ok().or(Some(candidate)))
        .unwrap_or_else(|| program.to_owned())
}

struct FingerprintInputs<'a> {
    executable: &'a ExecutableIdentity,
    driver_program: &'a Path,
    version: &'a str,
    reported_target: &'a Triple,
    target_arguments: &'a [OsString],
    sysroot: Option<&'a Path>,
    resource_dir: Option<&'a Path>,
    system_includes: &'a [SystemIncludeEntry],
    requirements: ToolchainRequirements,
    environment: &'a [EnvironmentEntry],
    working_directory: Option<&'a Path>,
}

fn fingerprint_digest(inputs: &FingerprintInputs<'_>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut update = |value: &OsStr| {
        for byte in value.as_encoded_bytes().iter().copied().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    update(inputs.executable.path.as_os_str());
    update(inputs.driver_program.as_os_str());
    if let Some(length) = inputs.executable.length {
        update(OsStr::new("length"));
        update(OsStr::new(&length.to_string()));
    }
    if let Some(modified) = inputs.executable.modified {
        update(OsStr::new("modified"));
        update(OsStr::new(if modified.before_epoch {
            "before-epoch"
        } else {
            "after-epoch"
        }));
        update(OsStr::new(&modified.seconds.to_string()));
        update(OsStr::new(&modified.nanoseconds.to_string()));
    }
    for value in &inputs.executable.platform_metadata {
        update(OsStr::new("platform-metadata"));
        update(OsStr::new(&value.to_string()));
    }
    update(OsStr::new(inputs.version));
    update(OsStr::new(&inputs.reported_target.to_string()));
    for argument in inputs.target_arguments {
        update(argument);
    }
    if let Some(sysroot) = inputs.sysroot {
        update(sysroot.as_os_str());
    }
    if let Some(resource_dir) = inputs.resource_dir {
        update(resource_dir.as_os_str());
    }
    for entry in inputs.system_includes {
        update(entry.path.as_os_str());
        update(OsStr::new(match entry.kind {
            SystemIncludeKind::Quote => "quote",
            SystemIncludeKind::Builtin => "builtin",
            SystemIncludeKind::System => "system",
            SystemIncludeKind::Framework => "framework",
            SystemIncludeKind::After => "after",
        }));
    }
    for (name, enabled) in [
        ("system-headers", inputs.requirements.system_headers),
        (
            "disable-system-headers",
            inputs.requirements.disable_system_headers,
        ),
        ("assembler", inputs.requirements.assembler),
        ("linker", inputs.requirements.linker),
        ("object-copier", inputs.requirements.object_copier),
        ("archiver", inputs.requirements.archiver),
    ] {
        update(OsStr::new(name));
        update(OsStr::new(if enabled { "enabled" } else { "disabled" }));
    }
    for entry in inputs.environment {
        update(OsStr::new(entry.name));
        match &entry.value {
            Some(value) => {
                update(OsStr::new("present"));
                update(value);
            }
            None => update(OsStr::new("absent")),
        }
    }
    update(OsStr::new("working-directory"));
    match inputs.working_directory {
        Some(path) => {
            update(OsStr::new("present"));
            update(path.as_os_str());
        }
        None => update(OsStr::new("absent")),
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::*;

    #[test]
    fn recognizes_common_spellings_of_the_primary_target() {
        let expected: Triple = "x86_64-unknown-linux-gnu".parse().unwrap();
        assert!(target_matches(
            &"x86_64-linux-gnu".parse().unwrap(),
            &expected
        ));
        assert!(target_matches(
            &"x86_64-redhat-linux-gnu".parse().unwrap(),
            &expected
        ));
        assert!(!target_matches(
            &"x86_64-apple-darwin".parse().unwrap(),
            &expected
        ));
        let macos: Triple = "aarch64-apple-darwin".parse().unwrap();
        assert!(target_matches(
            &"aarch64-apple-macosx".parse().unwrap(),
            &macos
        ));
        assert!(!target_matches(
            &"aarch64-apple-ios".parse().unwrap(),
            &macos
        ));
    }

    #[test]
    fn splits_a_compiler_driver_from_its_leading_arguments() {
        let command = parse_driver_command(OsString::from("ccache cc -m64")).unwrap();
        assert_eq!(command.program, PathBuf::from("ccache"));
        assert_eq!(
            command.arguments,
            [OsString::from("cc"), OsString::from("-m64")]
        );
    }

    #[test]
    fn parses_gcc_include_search_output() {
        let output = include_str!("../testdata/gcc-include-search.txt");
        assert_eq!(
            parse_system_include_search(output, Some(Path::new("/sdk"))),
            [
                SystemIncludeEntry::new("/work/project/include", SystemIncludeKind::Quote),
                SystemIncludeEntry::new(
                    "/usr/lib/gcc/x86_64-linux-gnu/14/include",
                    SystemIncludeKind::System,
                ),
                SystemIncludeEntry::new("/sdk/usr/local/include", SystemIncludeKind::System),
                SystemIncludeEntry::new("/sdk/usr/include", SystemIncludeKind::System),
            ]
        );
    }

    #[test]
    fn parses_clang_include_search_output() {
        let output = include_str!("../testdata/clang-include-search.txt");
        assert_eq!(
            parse_system_include_search(output, None),
            [
                SystemIncludeEntry::new(
                    "/opt/clang/lib/clang/20/include",
                    SystemIncludeKind::System,
                ),
                SystemIncludeEntry::new("/usr/local/include", SystemIncludeKind::System),
                SystemIncludeEntry::new("/System/Library/Frameworks", SystemIncludeKind::Framework,),
            ]
        );
    }

    #[test]
    fn parses_clang_effective_sysroot_traces() {
        let native = r#" "/usr/bin/clang" "-cc1" "-triple" "x86_64-pc-linux-gnu" "-E""#;
        assert_eq!(
            parse_clang_sysroot_trace(native),
            Some(ClangSysrootTrace::Default)
        );

        let configured = r#" "/usr/bin/clang" "-cc1" "-isysroot" "/sdk with space" "-E""#;
        assert_eq!(
            parse_clang_sysroot_trace(configured),
            Some(ClangSysrootTrace::Path(PathBuf::from("/sdk with space")))
        );

        let escaped = r#" "/usr/bin/clang" "-cc1" "-isysroot" "/sdk\\root" "-E""#;
        assert_eq!(
            parse_clang_sysroot_trace(escaped),
            Some(ClangSysrootTrace::Path(PathBuf::from(r"/sdk\root")))
        );
        assert_eq!(
            parse_clang_sysroot_trace("Target: x86_64-pc-linux-gnu"),
            None
        );
    }

    #[derive(Debug)]
    struct FakeRunner {
        requests: RefCell<Vec<ProbeRequest>>,
        outputs: RefCell<VecDeque<ProbeOutput>>,
    }

    impl FakeRunner {
        fn new(outputs: impl IntoIterator<Item = ProbeOutput>) -> Self {
            Self {
                requests: RefCell::new(Vec::new()),
                outputs: RefCell::new(outputs.into_iter().collect()),
            }
        }
    }

    impl ProbeRunner for FakeRunner {
        fn run(&self, request: &ProbeRequest) -> io::Result<ProbeOutput> {
            self.requests.borrow_mut().push(request.clone());
            Ok(self
                .outputs
                .borrow_mut()
                .pop_front()
                .expect("fake probe output"))
        }
    }

    fn successful(stdout: &str) -> ProbeOutput {
        ProbeOutput {
            success: true,
            status: "exit status: 0".to_owned(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn successful_with_stderr(stderr: &str) -> ProbeOutput {
        ProbeOutput {
            success: true,
            status: "exit status: 0".to_owned(),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn fixture_executable_identity() -> ExecutableIdentity {
        ExecutableIdentity {
            path: PathBuf::from("/tools/fixture-cc"),
            length: Some(4_096),
            modified: Some(FileTimestamp {
                before_epoch: false,
                seconds: 1_700_000_000,
                nanoseconds: 123,
            }),
            platform_metadata: vec![7, 42, 0o755],
        }
    }

    fn fixture_environment() -> Vec<EnvironmentEntry> {
        relevant_environment_with(|name| match name {
            "PATH" => Some(OsString::from("/tools:/usr/bin")),
            "SDKROOT" => Some(OsString::from("/sdk")),
            _ => None,
        })
    }

    #[test]
    fn process_cache_reuses_a_successful_resolution() {
        let cache = Mutex::new(ResolutionCache::new(4));
        let target: Triple = "x86_64-unknown-linux-gnu".parse().unwrap();
        let existing = ToolchainSpec::default();
        let candidate = ToolCommandSpec::new("fixture-cc");
        let executable = fixture_executable_identity();
        let environment = fixture_environment();
        let key = resolution_cache_key(&ResolutionInputs {
            target: &target,
            existing: &existing,
            candidate: &candidate,
            target_arguments: &[],
            explicit_sysroot: None,
            explicit_resource_dir: None,
            requirements: ToolchainRequirements::preprocess(false),
            executable: &executable,
            driver_version: "fixture cc 1.0",
            environment: &environment,
            working_directory: Some(Path::new("/work")),
        });
        let expected = ToolchainSpec {
            compiler_driver: Some(ToolCommandSpec::new("fixture-cc")),
            ..ToolchainSpec::default()
        };
        let calls = Cell::new(0);
        let unstable_key = key.clone();

        let first = resolve_with_cache(
            &cache,
            key.clone(),
            || {
                calls.set(calls.get() + 1);
                Ok(expected.clone())
            },
            || true,
        )
        .unwrap();
        let second = resolve_with_cache(
            &cache,
            key.clone(),
            || {
                calls.set(calls.get() + 1);
                Ok(ToolchainSpec::default())
            },
            || true,
        )
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(first, expected);
        assert_eq!(second, expected);

        let mut changed_version_key = key;
        changed_version_key.driver_version = "fixture cc 2.0".to_owned();
        resolve_with_cache(
            &cache,
            changed_version_key,
            || {
                calls.set(calls.get() + 1);
                Ok(ToolchainSpec::default())
            },
            || true,
        )
        .unwrap();
        assert_eq!(calls.get(), 2);

        let unstable_cache = Mutex::new(ResolutionCache::new(4));
        calls.set(0);
        resolve_with_cache(
            &unstable_cache,
            unstable_key.clone(),
            || {
                calls.set(calls.get() + 1);
                Ok(expected.clone())
            },
            || false,
        )
        .unwrap();
        resolve_with_cache(
            &unstable_cache,
            unstable_key,
            || {
                calls.set(calls.get() + 1);
                Ok(expected.clone())
            },
            || true,
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn environment_and_executable_identity_change_cache_keys_and_fingerprints() {
        let target: Triple = "x86_64-unknown-linux-gnu".parse().unwrap();
        let candidate = ToolCommandSpec::with_arguments("fixture-cc", ["--driver-mode=gcc"]);
        let requirements = ToolchainRequirements::preprocess(false);
        let identity = fixture_executable_identity();
        let environment = fixture_environment();
        let existing = ToolchainSpec::default();
        let target_arguments = [OsString::from("--target=x86_64-linux-gnu")];
        let key = resolution_cache_key(&ResolutionInputs {
            target: &target,
            existing: &existing,
            candidate: &candidate,
            target_arguments: &target_arguments,
            explicit_sysroot: Some(Path::new("/sdk")),
            explicit_resource_dir: Some(Path::new("/tools/lib/clang/20")),
            requirements,
            executable: &identity,
            driver_version: "fixture cc 1.0",
            environment: &environment,
            working_directory: Some(Path::new("/work")),
        });
        let digest = fingerprint_digest(&FingerprintInputs {
            executable: &identity,
            driver_program: &candidate.program,
            version: "fixture cc 1.0",
            reported_target: &target,
            target_arguments: &candidate.arguments,
            sysroot: Some(Path::new("/sdk")),
            resource_dir: Some(Path::new("/tools/lib/clang/20")),
            system_includes: &[],
            requirements,
            environment: &environment,
            working_directory: Some(Path::new("/work")),
        });

        let mut changed_environment = environment.clone();
        changed_environment
            .iter_mut()
            .find(|entry| entry.name == "SDKROOT")
            .unwrap()
            .value = Some(OsString::from("/other-sdk"));
        let environment_key = resolution_cache_key(&ResolutionInputs {
            target: &target,
            existing: &existing,
            candidate: &candidate,
            target_arguments: &target_arguments,
            explicit_sysroot: Some(Path::new("/sdk")),
            explicit_resource_dir: Some(Path::new("/tools/lib/clang/20")),
            requirements,
            executable: &identity,
            driver_version: "fixture cc 1.0",
            environment: &changed_environment,
            working_directory: Some(Path::new("/work")),
        });
        let environment_digest = fingerprint_digest(&FingerprintInputs {
            executable: &identity,
            driver_program: &candidate.program,
            version: "fixture cc 1.0",
            reported_target: &target,
            target_arguments: &candidate.arguments,
            sysroot: Some(Path::new("/sdk")),
            resource_dir: Some(Path::new("/tools/lib/clang/20")),
            system_includes: &[],
            requirements,
            environment: &changed_environment,
            working_directory: Some(Path::new("/work")),
        });

        let mut changed_identity = identity.clone();
        changed_identity.length = Some(identity.length.unwrap() + 1);
        let identity_key = resolution_cache_key(&ResolutionInputs {
            target: &target,
            existing: &existing,
            candidate: &candidate,
            target_arguments: &target_arguments,
            explicit_sysroot: Some(Path::new("/sdk")),
            explicit_resource_dir: Some(Path::new("/tools/lib/clang/20")),
            requirements,
            executable: &changed_identity,
            driver_version: "fixture cc 1.0",
            environment: &environment,
            working_directory: Some(Path::new("/work")),
        });
        let identity_digest = fingerprint_digest(&FingerprintInputs {
            executable: &changed_identity,
            driver_program: &candidate.program,
            version: "fixture cc 1.0",
            reported_target: &target,
            target_arguments: &candidate.arguments,
            sysroot: Some(Path::new("/sdk")),
            resource_dir: Some(Path::new("/tools/lib/clang/20")),
            system_includes: &[],
            requirements,
            environment: &environment,
            working_directory: Some(Path::new("/work")),
        });

        assert_ne!(key, environment_key);
        assert_ne!(digest, environment_digest);
        assert_ne!(key, identity_key);
        assert_ne!(digest, identity_digest);
    }

    #[test]
    fn only_the_process_backed_constructor_enables_the_shared_cache() {
        let config = EffectiveCompilationConfig::default();
        assert!(ToolchainResolver::new(&config).cache_process_resolution);

        let injected = ToolchainResolver::with_runner(&config, FakeRunner::new([]));
        assert!(!injected.cache_process_resolution);
    }

    #[test]
    fn injected_runners_are_invoked_for_every_resolution() {
        let config = EffectiveCompilationConfig::default();
        let runner = FakeRunner::new([
            successful("x86_64-linux-gnu\n"),
            successful("fixture cc 1.0\n"),
            successful("x86_64-linux-gnu\n"),
            successful("fixture cc 1.0\n"),
        ]);
        let resolver = ToolchainResolver::with_runner(&config, runner)
            .driver(ToolCommandSpec::new("fixture-cc"));

        resolver
            .resolve(ToolchainRequirements::preprocess(false))
            .unwrap();
        resolver
            .resolve(ToolchainRequirements::preprocess(false))
            .unwrap();

        assert_eq!(resolver.runner.requests.borrow().len(), 4);
    }

    #[test]
    fn compiler_resources_do_not_replace_toolchain_resources() {
        let mut config = EffectiveCompilationConfig::default().with_resource_dir("/ccc/resources");
        config.toolchain.resource_dir = Some(PathBuf::from("/toolchain/resources"));
        let resolver = ToolchainResolver::with_runner(&config, FakeRunner::new([]));

        assert_eq!(
            resolver.explicit_resource_dir.as_deref(),
            Some(Path::new("/toolchain/resources"))
        );
    }

    #[test]
    fn disabling_default_includes_skips_sysroot_and_include_probes() {
        let config = EffectiveCompilationConfig::default();
        let runner = FakeRunner::new([
            successful("x86_64-linux-gnu\n"),
            successful("fixture cc 1.0\n"),
        ]);
        let resolver = ToolchainResolver::with_runner(&config, runner)
            .driver(ToolCommandSpec::new("fixture-cc"));

        let spec = resolver
            .resolve(ToolchainRequirements::preprocess(false))
            .unwrap();

        let requests = resolver.runner.requests.borrow();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            !request.arguments.iter().any(|argument| {
                argument == "--print-sysroot" || argument == "-v" || argument == "-E"
            })
        }));
        assert!(spec.sysroot.is_none());
        assert!(spec.system_includes.is_empty());
        assert!(spec.fingerprint.is_some());
    }

    #[test]
    fn resolves_and_fingerprints_clang_shaped_system_paths() {
        let config = EffectiveCompilationConfig::default();
        let runner = FakeRunner::new([
            successful("x86_64-linux-gnu\n"),
            successful("fixture clang version 20.1.0\n"),
            successful_with_stderr(
                r#" "/opt/clang/bin/clang" "-cc1" "-triple" "x86_64-unknown-linux-gnu" "-isysroot" "/sdk" "-E" "-x" "c" "-"
"#,
            ),
            successful("/opt/clang/lib/clang/20\n"),
            successful_with_stderr(include_str!("../testdata/clang-include-search.txt")),
        ]);
        let resolver = ToolchainResolver::with_runner(&config, runner)
            .driver(ToolCommandSpec::new("fixture-clang"));

        let spec = resolver
            .resolve(ToolchainRequirements::preprocess(true))
            .unwrap();

        assert_eq!(spec.sysroot.as_deref(), Some(Path::new("/sdk")));
        assert_eq!(
            spec.resource_dir.as_deref(),
            Some(Path::new("/opt/clang/lib/clang/20"))
        );
        assert_eq!(spec.system_includes.len(), 3);
        assert_eq!(spec.system_includes[0].kind, SystemIncludeKind::Builtin);
        let fingerprint = spec.fingerprint.as_ref().unwrap();
        assert_eq!(
            fingerprint.reported_target.to_string(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(fingerprint.driver_version, "fixture clang version 20.1.0");
        assert!(fingerprint.digest.starts_with("fnv1a64:"));

        let requests = resolver.runner.requests.borrow();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[2].arguments,
            ["-march=x86-64", "-###", "-E", "-x", "c", "-"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        let include_probe = requests.last().unwrap();
        assert!(
            include_probe
                .arguments
                .iter()
                .any(|argument| argument == "-E")
        );
        assert!(
            include_probe
                .arguments
                .iter()
                .any(|argument| argument == "--sysroot=/sdk")
        );
        assert_eq!(include_probe.stdin, Some(Vec::new()));
    }

    #[test]
    fn resolves_gnu_sysroots_with_the_machine_readable_driver_query() {
        let config = EffectiveCompilationConfig::default();
        let runner = FakeRunner::new([
            successful("x86_64-linux-gnu\n"),
            successful("gcc (Debian 14.2.0) 14.2.0\n"),
            successful("/sdk\n"),
        ]);
        let resolver = ToolchainResolver::with_runner(&config, runner)
            .driver(ToolCommandSpec::new("fixture-gcc"));

        let spec = resolver.resolve(ToolchainRequirements::link()).unwrap();

        assert_eq!(spec.sysroot.as_deref(), Some(Path::new("/sdk")));
        assert_eq!(
            spec.linker_driver,
            Some(ToolCommandSpec::with_arguments(
                "fixture-gcc",
                [OsString::from("-march=x86-64")]
            ))
        );
        let requests = resolver.runner.requests.borrow();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[2].arguments.last(),
            Some(&OsString::from("--print-sysroot"))
        );
        assert!(
            requests[2]
                .arguments
                .iter()
                .all(|argument| argument != "-###")
        );
    }

    #[test]
    fn resolves_native_clang_when_the_gnu_sysroot_query_is_unsupported() {
        let config = EffectiveCompilationConfig::default();
        let runner = FakeRunner::new([
            successful("x86_64-pc-linux-gnu\n"),
            successful("Debian clang version 14.0.6\n"),
            successful_with_stderr(
                r#"Debian clang version 14.0.6
 "/usr/lib/llvm-14/bin/clang" "-cc1" "-triple" "x86_64-pc-linux-gnu" "-E" "-resource-dir" "/usr/lib/llvm-14/lib/clang/14.0.6" "-x" "c" "-"
"#,
            ),
            successful("/opt/clang/lib/clang/20\n"),
            successful_with_stderr(include_str!("../testdata/clang-include-search.txt")),
        ]);
        let resolver = ToolchainResolver::with_runner(&config, runner)
            .driver(ToolCommandSpec::new("fixture-clang"));

        let spec = resolver
            .resolve(ToolchainRequirements::preprocess(true))
            .unwrap();

        assert!(spec.sysroot.is_none());
        assert_eq!(spec.system_includes.len(), 3);

        let requests = resolver.runner.requests.borrow();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[2].arguments,
            ["-march=x86-64", "-###", "-E", "-x", "c", "-"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(requests[2].stdin, Some(Vec::new()));
        assert!(
            requests[4]
                .arguments
                .iter()
                .all(|argument| !argument.to_string_lossy().starts_with("--sysroot="))
        );
    }
}
