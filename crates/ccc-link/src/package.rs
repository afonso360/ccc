//! Late, transactional materialization of verified artifact bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ccc_target::{BinaryFormat, EffectiveCompilationConfig, ToolCommandSpec, ToolchainSpec};
use object::SymbolScope;
use object::read::{Object as _, ObjectSection as _, ObjectSymbol as _};
use sha2::{Digest as _, Sha256};

use crate::artifact::{
    ArtifactBundle, GeneratedSymbolOwner, GeneratedSymbolVisibility, VerifiedArtifactBundle,
    canonical_symbol_name, parse_relocatable,
};
use crate::bridge::is_bridge_generated_symbol;
use crate::{
    LinkError, ProbeRequest, ProbeRunner, ProcessProbeRunner, ToolchainRequirements,
    ToolchainResolver, artifact_error,
};

static WORKSPACE_ID: AtomicU64 = AtomicU64::new(0);

/// Identity of the object copier that passed the complete packaging probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackagingToolIdentity {
    pub command: ToolCommandSpec,
    pub fingerprint: String,
}

/// Details of a successfully materialized relocatable artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackagingReport {
    pub used_generated_assembly: bool,
    pub object_copier: Option<PackagingToolIdentity>,
}

/// Verifies and atomically materializes an artifact. Tool discovery is skipped
/// entirely for bridge-free bundles.
pub fn package_artifact_bundle(
    bundle: ArtifactBundle,
    output: &Path,
    config: &EffectiveCompilationConfig,
) -> Result<PackagingReport, LinkError> {
    let verified = bundle.verify()?;
    if !verified.needs_packaging_tools() {
        publish_bridge_free(&verified, output)?;
        return Ok(PackagingReport {
            used_generated_assembly: false,
            object_copier: None,
        });
    }

    let requirements = if config.target.triple.binary_format == BinaryFormat::Macho {
        ToolchainRequirements::package_generated_macho_assembly()
    } else {
        ToolchainRequirements::package_generated_assembly()
    };
    let toolchain = ToolchainResolver::new(config).resolve(requirements)?;
    package_with_runner(
        &verified,
        output,
        config,
        &toolchain,
        &ProcessProbeRunner,
        object_copier_candidates(config, &toolchain),
    )
}

/// Injectable packaging boundary used by deterministic tool-runner tests.
pub fn package_artifact_bundle_with_runner<R: ProbeRunner>(
    bundle: ArtifactBundle,
    output: &Path,
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
    runner: &R,
) -> Result<PackagingReport, LinkError> {
    let verified = bundle.verify()?;
    if !verified.needs_packaging_tools() {
        publish_bridge_free(&verified, output)?;
        return Ok(PackagingReport {
            used_generated_assembly: false,
            object_copier: None,
        });
    }
    let candidates = if config.target.triple.binary_format == BinaryFormat::Macho {
        Vec::new()
    } else {
        vec![toolchain.object_copier.clone().ok_or_else(|| LinkError {
            code: "CCC5011",
            message: "resolved toolchain has no object copier for generated assembly".to_owned(),
        })?]
    };
    package_with_runner(&verified, output, config, toolchain, runner, candidates)
}

fn publish_bridge_free(bundle: &VerifiedArtifactBundle, output: &Path) -> Result<(), LinkError> {
    let workspace = ArtifactWorkspace::create(output)?;
    let final_object = workspace.path().join("final.o");
    write_file(&final_object, bundle.primary_object())?;
    inspect_final_object(&final_object, bundle)?;
    workspace.publish(&final_object, output)
}

fn package_with_runner<R: ProbeRunner>(
    bundle: &VerifiedArtifactBundle,
    output: &Path,
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
    runner: &R,
    candidates: Vec<ToolCommandSpec>,
) -> Result<PackagingReport, LinkError> {
    let driver = toolchain
        .compiler_driver
        .as_ref()
        .ok_or_else(|| LinkError {
            code: "CCC5011",
            message: format!(
                "resolved toolchain for target `{}` has no compiler driver for generated assembly",
                config.target.triple
            ),
        })?;
    let workspace = ArtifactWorkspace::create(output)?;
    let macho = config.target.triple.binary_format == BinaryFormat::Macho;
    let copier = if macho {
        None
    } else {
        Some(probe_packaging_capabilities(
            workspace.path(),
            driver,
            &candidates,
            runner,
        )?)
    };

    let primary = workspace.path().join("primary.o");
    write_file(&primary, bundle.primary_object())?;
    inspect_path(&primary, "primary object")?;

    let mut objects = vec![primary];
    for assembly in bundle.assemblies() {
        let source = workspace.path().join(format!("{}.s", assembly.stem()));
        let object = workspace.path().join(format!("{}.o", assembly.stem()));
        write_file(&source, assembly.source().as_bytes())?;
        assemble(
            runner,
            driver,
            &source,
            &object,
            "generated bridge assembly",
        )?;
        inspect_generated_bridge_object(&object)?;
        objects.push(object);
    }

    let combined = workspace.path().join("combined.unlocalized.o");
    partial_link(runner, driver, &objects, &combined)?;
    inspect_combined_object(&combined, bundle, false)?;

    if macho {
        inspect_combined_object(&combined, bundle, true)?;
        workspace.publish(&combined, output)?;
        return Ok(PackagingReport {
            used_generated_assembly: true,
            object_copier: None,
        });
    }

    let localization_file = workspace.path().join("localize-symbols.txt");
    let mut localization = bundle.manifest().localization_symbols().join("\n");
    if !localization.is_empty() {
        localization.push('\n');
    }
    write_file(&localization_file, localization.as_bytes())?;

    let final_object = workspace.path().join("final.o");
    localize_symbols(
        runner,
        &copier
            .as_ref()
            .expect("non-Mach-O packaging probes an object copier")
            .command,
        &localization_file,
        &combined,
        &final_object,
    )?;
    inspect_combined_object(&final_object, bundle, true)?;
    workspace.publish(&final_object, output)?;
    Ok(PackagingReport {
        used_generated_assembly: true,
        object_copier: copier,
    })
}

fn probe_packaging_capabilities<R: ProbeRunner>(
    workspace: &Path,
    driver: &ToolCommandSpec,
    candidates: &[ToolCommandSpec],
    runner: &R,
) -> Result<PackagingToolIdentity, LinkError> {
    let probe_primary_source = workspace.join("capability-primary.s");
    let probe_generated_source = workspace.join("capability-generated.s");
    let probe_primary = workspace.join("capability-primary.o");
    let probe_generated = workspace.join("capability-generated.o");
    let probe_combined = workspace.join("capability-combined.o");
    let localization_file = workspace.join("capability-localize.txt");
    write_file(
        &probe_primary_source,
        b".text\n.globl __ccc_capability_primary\n.type __ccc_capability_primary,@function\n__ccc_capability_primary:\nret\n.section .note.GNU-stack,\"\",@progbits\n",
    )?;
    write_file(
        &probe_generated_source,
        b".text\n.globl __ccc_capability_internal\n.type __ccc_capability_internal,@function\n__ccc_capability_internal:\nret\n.section .note.GNU-stack,\"\",@progbits\n",
    )?;
    write_file(&localization_file, b"__ccc_capability_internal\n")?;
    assemble(
        runner,
        driver,
        &probe_primary_source,
        &probe_primary,
        "packaging capability primary",
    )?;
    assemble(
        runner,
        driver,
        &probe_generated_source,
        &probe_generated,
        "packaging capability assembly",
    )?;
    partial_link(
        runner,
        driver,
        &[probe_primary, probe_generated],
        &probe_combined,
    )?;
    inspect_path(&probe_combined, "packaging capability partial link")?;

    let mut failures = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let output = workspace.join(format!("capability-localized-{index}.o"));
        match localize_symbols(
            runner,
            candidate,
            &localization_file,
            &probe_combined,
            &output,
        )
        .and_then(|()| inspect_localized_probe(&output))
        {
            Ok(()) => {
                let fingerprint = fingerprint_tool(runner, candidate)?;
                return Ok(PackagingToolIdentity {
                    command: candidate.clone(),
                    fingerprint,
                });
            }
            Err(error) => failures.push(format!("{}: {}", candidate.display(), error.message)),
        }
    }
    Err(LinkError {
        code: "CCC5012",
        message: format!(
            "no object copier supports exact x86-64 ELF symbol localization: {}",
            failures.join("; ")
        ),
    })
}

fn assemble<R: ProbeRunner>(
    runner: &R,
    driver: &ToolCommandSpec,
    source: &Path,
    output: &Path,
    description: &str,
) -> Result<(), LinkError> {
    run_tool(
        runner,
        driver,
        vec![
            OsString::from("-x"),
            OsString::from("assembler"),
            OsString::from("-c"),
            source.as_os_str().to_owned(),
            OsString::from("-o"),
            output.as_os_str().to_owned(),
        ],
        description,
    )
}

fn partial_link<R: ProbeRunner>(
    runner: &R,
    driver: &ToolCommandSpec,
    objects: &[PathBuf],
    output: &Path,
) -> Result<(), LinkError> {
    let mut arguments = vec![OsString::from("-nostdlib"), OsString::from("-r")];
    arguments.extend(objects.iter().map(|path| path.as_os_str().to_owned()));
    arguments.extend([OsString::from("-o"), output.as_os_str().to_owned()]);
    run_tool(runner, driver, arguments, "generated-object partial link")
}

fn localize_symbols<R: ProbeRunner>(
    runner: &R,
    copier: &ToolCommandSpec,
    localization_file: &Path,
    input: &Path,
    output: &Path,
) -> Result<(), LinkError> {
    let mut option = OsString::from("--localize-symbols=");
    option.push(localization_file);
    run_tool(
        runner,
        copier,
        vec![
            option,
            input.as_os_str().to_owned(),
            output.as_os_str().to_owned(),
        ],
        "exact generated-symbol localization",
    )
}

fn run_tool<R: ProbeRunner>(
    runner: &R,
    command: &ToolCommandSpec,
    arguments: Vec<OsString>,
    description: &str,
) -> Result<(), LinkError> {
    let output = runner
        .run(&ProbeRequest {
            command: command.clone(),
            arguments,
            stdin: None,
        })
        .map_err(|error| LinkError {
            code: "CCC5013",
            message: format!(
                "cannot invoke `{}` for {description}: {error}",
                command.display()
            ),
        })?;
    if !output.success {
        return Err(LinkError {
            code: "CCC5014",
            message: format!(
                "`{}` failed during {description} with {}: {}",
                command.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(())
}

fn fingerprint_tool<R: ProbeRunner>(
    runner: &R,
    command: &ToolCommandSpec,
) -> Result<String, LinkError> {
    let output = runner
        .run(&ProbeRequest {
            command: command.clone(),
            arguments: vec![OsString::from("--version")],
            stdin: None,
        })
        .map_err(|error| LinkError {
            code: "CCC5013",
            message: format!(
                "cannot fingerprint object copier `{}`: {error}",
                command.display()
            ),
        })?;
    if !output.success {
        return Err(LinkError {
            code: "CCC5014",
            message: format!(
                "object copier `{}` rejected --version with {}",
                command.display(),
                output.status
            ),
        });
    }
    let mut digest = Sha256::new();
    digest.update(b"ccc-object-copier-v1\0");
    digest.update(command.program.as_os_str().as_encoded_bytes());
    for argument in &command.arguments {
        digest.update([0]);
        digest.update(argument.as_encoded_bytes());
    }
    if let Ok(metadata) = fs::metadata(&command.program) {
        digest.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            digest.update(duration.as_secs().to_le_bytes());
            digest.update(duration.subsec_nanos().to_le_bytes());
        }
    }
    digest.update(&output.stdout);
    digest.update(&output.stderr);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn inspect_path(path: &Path, description: &str) -> Result<(), LinkError> {
    let bytes = fs::read(path).map_err(|error| {
        artifact_error(format!(
            "cannot read {description} `{}`: {error}",
            path.display()
        ))
    })?;
    parse_relocatable(&bytes, description).map(|_| ())
}

fn inspect_generated_bridge_object(path: &Path) -> Result<(), LinkError> {
    let bytes = fs::read(path).map_err(|error| {
        artifact_error(format!(
            "cannot read generated bridge object `{}`: {error}",
            path.display()
        ))
    })?;
    let object = parse_relocatable(&bytes, "generated bridge object")?;
    if object
        .symbols()
        .any(|symbol| symbol.kind() == object::SymbolKind::File)
    {
        return Err(artifact_error(
            "generated bridge object contains an assembler file symbol",
        ));
    }
    if object
        .sections()
        .filter_map(|section| section.name().ok())
        .any(|name| name == ".gdb_index" || name.starts_with(".debug"))
    {
        return Err(artifact_error(
            "generated bridge object contains path-sensitive debug metadata",
        ));
    }
    Ok(())
}

fn inspect_localized_probe(path: &Path) -> Result<(), LinkError> {
    let bytes = fs::read(path)
        .map_err(|error| artifact_error(format!("cannot read localization probe: {error}")))?;
    let object = parse_relocatable(&bytes, "localized capability object")?;
    let symbol = object
        .symbols()
        .find(|symbol| symbol.name() == Ok("__ccc_capability_internal"))
        .ok_or_else(|| artifact_error("localized capability object lost its sentinel symbol"))?;
    if symbol.scope() != SymbolScope::Compilation {
        return Err(artifact_error(
            "object copier did not localize the sentinel symbol",
        ));
    }
    Ok(())
}

fn inspect_final_object(path: &Path, bundle: &VerifiedArtifactBundle) -> Result<(), LinkError> {
    inspect_combined_object(path, bundle, true)
}

fn inspect_combined_object(
    path: &Path,
    bundle: &VerifiedArtifactBundle,
    localized: bool,
) -> Result<(), LinkError> {
    let bytes = fs::read(path).map_err(|error| {
        artifact_error(format!(
            "cannot read packaged object `{}`: {error}",
            path.display()
        ))
    })?;
    let object = parse_relocatable(&bytes, "packaged object")?;
    #[derive(Clone, Copy)]
    struct SymbolFacts {
        scope: SymbolScope,
        kind: object::SymbolKind,
        weak: bool,
        elf_visibility: Option<u8>,
    }
    let mut symbols = BTreeMap::new();
    for symbol in object.symbols() {
        let name = symbol
            .name()
            .ok()
            .map(|name| canonical_symbol_name(object.format(), name));
        if symbol.is_undefined() && name.is_some_and(is_bridge_generated_symbol) {
            return Err(artifact_error(format!(
                "packaged object retains unresolved generated symbol `{}`",
                name.unwrap_or("<invalid>")
            )));
        }
        if symbol.is_undefined() {
            continue;
        }
        if matches!(
            symbol.kind(),
            object::SymbolKind::File | object::SymbolKind::Section
        ) {
            continue;
        }
        if let Some(name) = name
            && !name.is_empty()
        {
            symbols.insert(
                name.to_owned(),
                SymbolFacts {
                    scope: symbol.scope(),
                    kind: symbol.kind(),
                    weak: symbol.is_weak(),
                    elf_visibility: symbol.flags().elf_visibility(),
                },
            );
        }
    }
    if bundle.needs_packaging_tools() {
        let is_elf = object.format() == object::BinaryFormat::Elf;
        if object.section_by_name(".eh_frame").is_none()
            && object.section_by_name("__eh_frame").is_none()
        {
            return Err(artifact_error(
                "packaged bridge object is missing unwind information",
            ));
        }
        if is_elf {
            let stack_note = object.section_by_name(".note.GNU-stack").ok_or_else(|| {
                artifact_error("packaged bridge object is missing `.note.GNU-stack`")
            })?;
            if matches!(
                stack_note.flags(),
                object::SectionFlags::Elf { sh_flags }
                    if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
            ) {
                return Err(artifact_error(
                    "packaged bridge object requests an executable process stack",
                ));
            }
        }
    }
    for forbidden in [
        "_start",
        "__libc_start_main",
        "__libc_csu_init",
        "__libc_csu_fini",
    ] {
        if symbols.contains_key(forbidden) {
            return Err(artifact_error(format!(
                "partial linking unexpectedly introduced startup symbol `{forbidden}`"
            )));
        }
    }
    let manifest_names = bundle
        .manifest()
        .symbols()
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<BTreeSet<_>>();
    for name in symbols
        .keys()
        .filter(|name| is_bridge_generated_symbol(name))
    {
        if !manifest_names.contains(name.as_str()) {
            return Err(artifact_error(format!(
                "packaged object contains unmanifested generated symbol `{name}`"
            )));
        }
    }
    for expected in bundle.manifest().symbols() {
        let facts = symbols.get(&expected.name).ok_or_else(|| {
            artifact_error(format!(
                "packaged object does not define manifest symbol `{}`",
                expected.name
            ))
        })?;
        if localized
            && expected.visibility == GeneratedSymbolVisibility::Internal
            && if object.format() == object::BinaryFormat::MachO {
                facts.scope == SymbolScope::Dynamic
            } else {
                facts.scope != SymbolScope::Compilation
            }
        {
            return Err(artifact_error(format!(
                "generated symbol `{}` was not localized",
                expected.name
            )));
        }
        if localized {
            match expected.visibility {
                GeneratedSymbolVisibility::SourceInternal
                    if if object.format() == object::BinaryFormat::MachO {
                        facts.scope == SymbolScope::Dynamic
                    } else {
                        facts.scope != SymbolScope::Compilation
                    } =>
                {
                    return Err(artifact_error(format!(
                        "source-internal symbol `{}` did not retain local binding",
                        expected.name
                    )));
                }
                GeneratedSymbolVisibility::Public
                | GeneratedSymbolVisibility::SourceHidden
                | GeneratedSymbolVisibility::SourceProtected
                | GeneratedSymbolVisibility::SourceElfInternal
                    if facts.scope == SymbolScope::Compilation =>
                {
                    return Err(artifact_error(format!(
                        "externally linked symbol `{}` was unexpectedly localized",
                        expected.name
                    )));
                }
                _ => {}
            }
            if object.format() == object::BinaryFormat::Elf
                && expected.visibility == GeneratedSymbolVisibility::SourceHidden
                && facts.elf_visibility != Some(object::elf::STV_HIDDEN)
            {
                return Err(artifact_error(format!(
                    "source-hidden symbol `{}` lost hidden ELF visibility",
                    expected.name
                )));
            }
            if object.format() == object::BinaryFormat::Elf
                && expected.visibility == GeneratedSymbolVisibility::SourceProtected
                && facts.elf_visibility != Some(object::elf::STV_PROTECTED)
            {
                return Err(artifact_error(format!(
                    "source-protected symbol `{}` lost protected ELF visibility",
                    expected.name
                )));
            }
            if object.format() == object::BinaryFormat::Elf
                && expected.visibility == GeneratedSymbolVisibility::SourceElfInternal
                && facts.elf_visibility != Some(object::elf::STV_INTERNAL)
            {
                return Err(artifact_error(format!(
                    "source-internal-visibility symbol `{}` lost internal ELF visibility",
                    expected.name
                )));
            }
            if object.format() == object::BinaryFormat::Elf
                && expected.visibility == GeneratedSymbolVisibility::Public
                && !matches!(facts.elf_visibility, None | Some(object::elf::STV_DEFAULT))
            {
                return Err(artifact_error(format!(
                    "public symbol `{}` unexpectedly has non-default ELF visibility",
                    expected.name
                )));
            }
        }
    }
    let primary = parse_relocatable(bundle.primary_object(), "primary object")?;
    for symbol in primary.symbols().filter(|symbol| !symbol.is_undefined()) {
        if matches!(
            symbol.kind(),
            object::SymbolKind::File | object::SymbolKind::Section
        ) {
            continue;
        }
        let Ok(name) = symbol.name() else {
            continue;
        };
        let name = canonical_symbol_name(primary.format(), name);
        if name.is_empty() {
            continue;
        }
        let packaged = symbols.get(name).ok_or_else(|| {
            artifact_error(format!(
                "partial linking discarded primary-object symbol `{name}`"
            ))
        })?;
        if packaged.kind != symbol.kind()
            || packaged.weak != symbol.is_weak()
            || packaged.elf_visibility.unwrap_or(object::elf::STV_DEFAULT)
                != symbol
                    .flags()
                    .elf_visibility()
                    .unwrap_or(object::elf::STV_DEFAULT)
        {
            return Err(artifact_error(format!(
                "partial linking changed the kind, weak binding, or visibility of primary-object symbol `{name}`: primary=({:?}, weak={}, visibility={:?}), packaged=({:?}, weak={}, visibility={:?})",
                symbol.kind(),
                symbol.is_weak(),
                symbol.flags().elf_visibility(),
                packaged.kind,
                packaged.weak,
                packaged.elf_visibility,
            )));
        }
        let intentionally_localized = bundle.manifest().symbols().iter().any(|generated| {
            generated.name == name
                && generated.owner == GeneratedSymbolOwner::PrimaryObject
                && generated.visibility == GeneratedSymbolVisibility::Internal
        });
        if !intentionally_localized && packaged.scope != symbol.scope() {
            return Err(artifact_error(format!(
                "partial linking changed the binding of primary-object symbol `{name}`"
            )));
        }
    }
    Ok(())
}

fn object_copier_candidates(
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
) -> Vec<ToolCommandSpec> {
    let explicit = env::var_os("CCC_OBJCOPY").or_else(|| env::var_os("OBJCOPY"));
    if explicit.is_some() {
        return toolchain.object_copier.iter().cloned().collect();
    }
    fallback_object_copier_candidates(config, toolchain)
}

fn fallback_object_copier_candidates(
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
) -> Vec<ToolCommandSpec> {
    let mut candidates = Vec::new();
    if let Some(copier) = &toolchain.object_copier {
        candidates.push(copier.clone());
    }
    let target = config.target.triple.to_string();
    let conventional_target = target.replace("-unknown-", "-");
    for program in [
        format!("{target}-objcopy"),
        format!("{conventional_target}-objcopy"),
        "llvm-objcopy".to_owned(),
        "objcopy".to_owned(),
    ] {
        candidates.push(ToolCommandSpec::new(program));
    }
    if let Some(driver) = &toolchain.compiler_driver
        && let Some(directory) = driver.program.parent()
        && directory != Path::new("")
    {
        for program in [
            format!("{target}-objcopy"),
            format!("{conventional_target}-objcopy"),
            "llvm-objcopy".to_owned(),
            "objcopy".to_owned(),
        ] {
            candidates.push(ToolCommandSpec::new(directory.join(program)));
        }
    }
    let mut seen = BTreeSet::new();
    candidates
        .retain(|candidate| seen.insert((candidate.program.clone(), candidate.arguments.clone())));
    candidates
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), LinkError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| artifact_error(format!("cannot create `{}`: {error}", path.display())))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| artifact_error(format!("cannot write `{}`: {error}", path.display())))
}

struct ArtifactWorkspace {
    path: PathBuf,
    published: bool,
}

impl ArtifactWorkspace {
    fn create(destination: &Path) -> Result<Self, LinkError> {
        let directory = destination.parent().unwrap_or_else(|| Path::new("."));
        let stem = destination
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("output");
        for _ in 0..100 {
            let id = WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".{stem}.ccc-artifact-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(artifact_error(format!(
                        "cannot create artifact workspace beside `{}`: {error}",
                        destination.display()
                    )));
                }
            }
        }
        Err(artifact_error(
            "cannot allocate a collision-free artifact workspace",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn publish(mut self, source: &Path, destination: &Path) -> Result<(), LinkError> {
        File::open(source)
            .and_then(|file| file.sync_all())
            .map_err(|error| artifact_error(format!("cannot sync packaged object: {error}")))?;
        fs::rename(source, destination).map_err(|error| {
            artifact_error(format!(
                "cannot atomically replace `{}`: {error}",
                destination.display()
            ))
        })?;
        self.published = true;
        // Publication is the commit point. A best-effort cleanup failure must
        // not turn a successfully replaced, fully verified destination into a
        // reported compilation failure.
        let _ = fs::remove_dir_all(&self.path);
        Ok(())
    }
}

impl Drop for ArtifactWorkspace {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use object::write::{Object, Relocation, StandardSection, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };

    use crate::ProbeOutput;
    use crate::artifact::{BridgeManifestV1, GeneratedSymbol, GeneratedSymbolOwner};
    use crate::bridge::{GeneratedSymbolKind, render_generic_call_helper};

    use super::*;

    fn make_object(symbols: &[(&str, bool, SymbolScope)]) -> Vec<u8> {
        let symbols = symbols
            .iter()
            .map(|(name, defined, scope)| (*name, *defined, *scope, None))
            .collect::<Vec<_>>();
        make_object_with_visibility(&symbols)
    }

    fn make_object_with_visibility(symbols: &[(&str, bool, SymbolScope, Option<u8>)]) -> Vec<u8> {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0xc3], 1);
        let eh_frame =
            object.add_section(Vec::new(), b".eh_frame".to_vec(), SectionKind::ReadOnlyData);
        object.append_section_data(eh_frame, &[0], 1);
        let stack_note = object.add_section(
            Vec::new(),
            b".note.GNU-stack".to_vec(),
            SectionKind::ReadOnlyData,
        );
        object.append_section_data(stack_note, &[0], 1);
        for (name, defined, scope, visibility) in symbols {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0,
                size: 0,
                kind: SymbolKind::Text,
                scope: *scope,
                weak: false,
                section: if *defined {
                    SymbolSection::Section(text)
                } else {
                    SymbolSection::Undefined
                },
                flags: visibility.map_or(SymbolFlags::None, |st_other| SymbolFlags::Elf {
                    st_info: ((if *scope == SymbolScope::Compilation {
                        object::elf::STB_LOCAL
                    } else {
                        object::elf::STB_GLOBAL
                    }) << 4)
                        | object::elf::STT_FUNC,
                    st_other,
                }),
            });
        }
        object.write().unwrap()
    }

    fn make_object_with_call_and_address_references(symbol: &str, hidden_body: &str) -> Vec<u8> {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0; 16], 8);
        let target = object.add_symbol(Symbol {
            name: symbol.as_bytes().to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Text,
            scope: SymbolScope::Unknown,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        object.add_symbol(Symbol {
            name: hidden_body.as_bytes().to_vec(),
            value: 0,
            size: 1,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        object
            .add_relocation(
                text,
                Relocation {
                    offset: 0,
                    symbol: target,
                    addend: -4,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_X86_64_PLT32,
                    },
                },
            )
            .unwrap();
        object
            .add_relocation(
                text,
                Relocation {
                    offset: 8,
                    symbol: target,
                    addend: 0,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_X86_64_64,
                    },
                },
            )
            .unwrap();
        object.write().unwrap()
    }

    #[derive(Default)]
    struct FakeRunner {
        requests: RefCell<Vec<ProbeRequest>>,
        fail_at: Cell<Option<usize>>,
    }

    impl FakeRunner {
        fn successful() -> Self {
            Self::default()
        }

        fn failing_at(index: usize) -> Self {
            Self {
                fail_at: Cell::new(Some(index)),
                ..Self::default()
            }
        }
    }

    impl ProbeRunner for FakeRunner {
        fn run(&self, request: &ProbeRequest) -> io::Result<ProbeOutput> {
            let index = self.requests.borrow().len();
            self.requests.borrow_mut().push(request.clone());
            if self.fail_at.get() == Some(index) {
                return Ok(ProbeOutput {
                    success: false,
                    status: "exit status: 1".to_owned(),
                    stdout: Vec::new(),
                    stderr: b"injected failure".to_vec(),
                });
            }
            if request.arguments == [OsString::from("--version")] {
                return Ok(ProbeOutput {
                    success: true,
                    status: "exit status: 0".to_owned(),
                    stdout: b"fake objcopy 1\n".to_vec(),
                    stderr: Vec::new(),
                });
            }
            emulate_tool(request)?;
            Ok(ProbeOutput {
                success: true,
                status: "exit status: 0".to_owned(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    struct MissingToolRunner {
        missing_program: &'static str,
    }

    impl ProbeRunner for MissingToolRunner {
        fn run(&self, request: &ProbeRequest) -> io::Result<ProbeOutput> {
            if request.command.program == Path::new(self.missing_program) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "injected missing executable",
                ));
            }
            FakeRunner::successful().run(request)
        }
    }

    fn emulate_tool(request: &ProbeRequest) -> io::Result<()> {
        let arguments = &request.arguments;
        if let Some(position) = arguments.iter().position(|argument| argument == "-c") {
            let source = PathBuf::from(&arguments[position + 1]);
            let output = output_path(arguments).unwrap();
            let text = fs::read_to_string(source)?;
            let names = text
                .lines()
                .filter_map(|line| line.strip_prefix(".globl "))
                .map(|name| {
                    let visibility = [
                        (".hidden ", object::elf::STV_HIDDEN),
                        (".protected ", object::elf::STV_PROTECTED),
                        (".internal ", object::elf::STV_INTERNAL),
                    ]
                    .into_iter()
                    .find_map(|(directive, visibility)| {
                        text.lines()
                            .any(|line| line == format!("{directive}{name}"))
                            .then_some(visibility)
                    });
                    (name, true, SymbolScope::Dynamic, visibility)
                })
                .collect::<Vec<_>>();
            return fs::write(output, make_object_with_visibility(&names));
        }
        if arguments.iter().any(|argument| argument == "-r") {
            let output = output_path(arguments).unwrap();
            let mut by_name = BTreeMap::<String, (bool, SymbolScope, Option<u8>)>::new();
            for path in arguments
                .iter()
                .filter(|argument| Path::new(argument).extension() == Some(OsStr::new("o")))
                .map(PathBuf::from)
                .filter(|path| path != &output)
            {
                let bytes = fs::read(path)?;
                let object = object::File::parse(bytes.as_slice()).unwrap();
                for symbol in object.symbols() {
                    if let Ok(name) = symbol.name()
                        && !name.is_empty()
                    {
                        let incoming = (
                            !symbol.is_undefined(),
                            symbol.scope(),
                            symbol.flags().elf_visibility(),
                        );
                        by_name
                            .entry(name.to_owned())
                            .and_modify(|current| {
                                if !current.0 && incoming.0 {
                                    *current = incoming;
                                }
                            })
                            .or_insert(incoming);
                    }
                }
            }
            let names = by_name
                .iter()
                .map(|(name, (defined, scope, visibility))| {
                    (name.as_str(), *defined, *scope, *visibility)
                })
                .collect::<Vec<_>>();
            return fs::write(output, make_object_with_visibility(&names));
        }
        if let Some(option) = arguments.iter().find(|argument| {
            argument
                .to_string_lossy()
                .starts_with("--localize-symbols=")
        }) {
            let option = option.to_string_lossy();
            let names = fs::read_to_string(option.trim_start_matches("--localize-symbols="))?
                .lines()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            let input = PathBuf::from(&arguments[arguments.len() - 2]);
            let output = PathBuf::from(&arguments[arguments.len() - 1]);
            let bytes = fs::read(input)?;
            let object = object::File::parse(bytes.as_slice()).unwrap();
            let symbols = object
                .symbols()
                .filter_map(|symbol| {
                    let name = symbol.name().ok()?;
                    (!name.is_empty()).then_some((
                        name.to_owned(),
                        !symbol.is_undefined(),
                        if names.contains(name) {
                            SymbolScope::Compilation
                        } else {
                            symbol.scope()
                        },
                        symbol.flags().elf_visibility(),
                    ))
                })
                .collect::<Vec<_>>();
            let borrowed = symbols
                .iter()
                .map(|(name, defined, scope, visibility)| {
                    (name.as_str(), *defined, *scope, *visibility)
                })
                .collect::<Vec<_>>();
            return fs::write(output, make_object_with_visibility(&borrowed));
        }
        Ok(())
    }

    fn output_path(arguments: &[OsString]) -> Option<PathBuf> {
        arguments
            .iter()
            .position(|argument| argument == "-o")
            .map(|position| PathBuf::from(&arguments[position + 1]))
    }

    fn test_directory(name: &str) -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "ccc-package-test-{}-{}-{name}",
            std::process::id(),
            WORKSPACE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn toolchain() -> ToolchainSpec {
        ToolchainSpec {
            compiler_driver: Some(ToolCommandSpec::new("fake-cc")),
            object_copier: Some(ToolCommandSpec::new("fake-objcopy")),
            ..ToolchainSpec::default()
        }
    }

    #[test]
    fn object_copier_fallbacks_cover_target_and_driver_adjacent_tools() {
        let mut toolchain = toolchain();
        toolchain.compiler_driver = Some(ToolCommandSpec::new("/tools/bin/clang"));
        let candidates = fallback_object_copier_candidates(
            &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
            &toolchain,
        )
        .into_iter()
        .map(|candidate| candidate.program)
        .collect::<BTreeSet<_>>();
        for expected in [
            PathBuf::from("x86_64-unknown-linux-gnu-objcopy"),
            PathBuf::from("x86_64-linux-gnu-objcopy"),
            PathBuf::from("llvm-objcopy"),
            PathBuf::from("objcopy"),
            PathBuf::from("/tools/bin/x86_64-unknown-linux-gnu-objcopy"),
            PathBuf::from("/tools/bin/llvm-objcopy"),
        ] {
            assert!(
                candidates.contains(&expected),
                "missing {}",
                expected.display()
            );
        }
    }

    #[test]
    fn bridge_free_publication_does_not_invoke_tools() {
        let directory = test_directory("bridge-free");
        let output = directory.join("result.o");
        let runner = FakeRunner::successful();
        package_artifact_bundle_with_runner(
            ArtifactBundle::bridge_free(
                make_object(&[("main", true, SymbolScope::Linkage)]),
                [0; 32],
            ),
            &output,
            &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
            &ToolchainSpec::default(),
            &runner,
        )
        .unwrap();
        assert!(runner.requests.borrow().is_empty());
        inspect_path(&output, "published object").unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_bridge_objects_reject_file_symbols() {
        let directory = test_directory("bridge-file-symbol");
        let object_path = directory.join("bridge.o");
        let mut object = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        object.add_file_symbol(b"/build/path/bridge.s".to_vec());
        fs::write(&object_path, object.write().unwrap()).unwrap();

        let error = inspect_generated_bridge_object(&object_path).unwrap_err();
        assert_eq!(error.code, "CCC5010");
        assert_eq!(
            error.message,
            "generated bridge object contains an assembler file symbol"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_assembly_uses_driver_partial_link_and_exact_localization() {
        let directory = test_directory("generated");
        let output = directory.join("result.o");
        let helper = "__ccc_call_helper_test";
        let assembly = render_generic_call_helper(helper).unwrap();
        let bundle = ArtifactBundle::new(
            make_object(&[
                (helper, false, SymbolScope::Unknown),
                ("__ccc_string_0", true, SymbolScope::Compilation),
            ]),
            vec![assembly],
            BridgeManifestV1::new(
                [1; 32],
                vec![GeneratedSymbol::internal(
                    helper,
                    GeneratedSymbolKind::CallHelper,
                    GeneratedSymbolOwner::AssemblyUnit("call-helper".to_owned()),
                )],
            ),
        );
        let runner = FakeRunner::successful();
        let report = package_artifact_bundle_with_runner(
            bundle,
            &output,
            &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
            &toolchain(),
            &runner,
        )
        .unwrap();
        assert!(report.used_generated_assembly);
        assert!(
            report
                .object_copier
                .unwrap()
                .fingerprint
                .starts_with("sha256:")
        );
        let requests = runner.requests.borrow();
        assert!(requests.iter().any(|request| {
            request.command.program == Path::new("fake-cc")
                && request.arguments.iter().any(|argument| argument == "-r")
        }));
        assert!(requests.iter().any(|request| {
            request.command.program == Path::new("fake-objcopy")
                && request.arguments.iter().any(|argument| {
                    argument
                        .to_string_lossy()
                        .starts_with("--localize-symbols=")
                })
        }));
        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = object
            .symbols()
            .find(|symbol| symbol.name() == Ok(helper))
            .unwrap();
        assert_eq!(symbol.scope(), SymbolScope::Compilation);
        let string = object
            .symbols()
            .find(|symbol| symbol.name() == Ok("__ccc_string_0"))
            .unwrap();
        assert_eq!(string.scope(), SymbolScope::Compilation);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn packaged_variadic_entries_keep_all_external_elf_visibilities() {
        use crate::bridge::{AssemblyFunctionLinkage, VariadicEntryPlan, render_variadic_entry};

        let directory = test_directory("entry-visibilities");
        let output = directory.join("result.o");
        let entries = [
            (
                "variadic_default",
                AssemblyFunctionLinkage::ExternalDefault,
                object::elf::STV_DEFAULT,
            ),
            (
                "variadic_hidden",
                AssemblyFunctionLinkage::ExternalHidden,
                object::elf::STV_HIDDEN,
            ),
            (
                "variadic_protected",
                AssemblyFunctionLinkage::ExternalProtected,
                object::elf::STV_PROTECTED,
            ),
            (
                "variadic_internal",
                AssemblyFunctionLinkage::ExternalInternal,
                object::elf::STV_INTERNAL,
            ),
        ];
        let mut assemblies = Vec::new();
        let mut manifest_symbols = Vec::new();
        let mut primary_symbols = Vec::new();
        for (index, (public, linkage, _)) in entries.iter().enumerate() {
            let body = format!("__ccc_variadic_body_visibility_{index}");
            let assembly = render_variadic_entry(&VariadicEntryPlan {
                public_symbol: (*public).to_owned(),
                hidden_body_symbol: body.clone(),
                linkage: *linkage,
                fixed_gp_used: 1,
                fixed_sse_used: 0,
                overflow_arg_offset: 0,
                gp_results: 1,
                xmm_results: 0,
                hidden_return: false,
                logical_line: 1,
            })
            .unwrap();
            let owner = GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned());
            let entry = match linkage {
                AssemblyFunctionLinkage::ExternalDefault => {
                    GeneratedSymbol::public(*public, GeneratedSymbolKind::VariadicEntry, owner)
                }
                AssemblyFunctionLinkage::ExternalHidden => GeneratedSymbol::source_hidden(
                    *public,
                    GeneratedSymbolKind::VariadicEntry,
                    owner,
                ),
                AssemblyFunctionLinkage::ExternalProtected => GeneratedSymbol::source_protected(
                    *public,
                    GeneratedSymbolKind::VariadicEntry,
                    owner,
                ),
                AssemblyFunctionLinkage::ExternalInternal => GeneratedSymbol::source_elf_internal(
                    *public,
                    GeneratedSymbolKind::VariadicEntry,
                    owner,
                ),
                AssemblyFunctionLinkage::Internal => unreachable!(),
            };
            manifest_symbols.push(entry);
            manifest_symbols.push(GeneratedSymbol::internal(
                &body,
                GeneratedSymbolKind::VariadicBody,
                GeneratedSymbolOwner::PrimaryObject,
            ));
            primary_symbols.push(((*public).to_owned(), false, SymbolScope::Unknown));
            primary_symbols.push((body, true, SymbolScope::Linkage));
            assemblies.push(assembly);
        }
        let primary_symbols = primary_symbols
            .iter()
            .map(|(name, defined, scope)| (name.as_str(), *defined, *scope))
            .collect::<Vec<_>>();
        package_artifact_bundle_with_runner(
            ArtifactBundle::new(
                make_object(&primary_symbols),
                assemblies,
                BridgeManifestV1::new([6; 32], manifest_symbols),
            ),
            &output,
            &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
            &toolchain(),
            &FakeRunner::successful(),
        )
        .unwrap();

        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        for (name, _, visibility) in entries {
            let symbol = object
                .symbols()
                .find(|symbol| symbol.name() == Ok(name))
                .unwrap();
            assert_ne!(symbol.scope(), SymbolScope::Compilation, "{name}");
            assert_eq!(symbol.flags().elf_visibility(), Some(visibility), "{name}");
        }
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn every_tool_phase_failure_preserves_the_destination_and_cleans_intermediates() {
        // Capability primary assembly, capability generated assembly,
        // capability partial link, capability localization, copier identity,
        // real assembly, real partial link, and real localization.
        let expected = [
            (
                "CCC5014",
                "`fake-cc` failed during packaging capability primary with exit status: 1: injected failure",
            ),
            (
                "CCC5014",
                "`fake-cc` failed during packaging capability assembly with exit status: 1: injected failure",
            ),
            (
                "CCC5014",
                "`fake-cc` failed during generated-object partial link with exit status: 1: injected failure",
            ),
            (
                "CCC5012",
                "no object copier supports exact x86-64 ELF symbol localization: fake-objcopy: `fake-objcopy` failed during exact generated-symbol localization with exit status: 1: injected failure",
            ),
            (
                "CCC5014",
                "object copier `fake-objcopy` rejected --version with exit status: 1",
            ),
            (
                "CCC5014",
                "`fake-cc` failed during generated bridge assembly with exit status: 1: injected failure",
            ),
            (
                "CCC5014",
                "`fake-cc` failed during generated-object partial link with exit status: 1: injected failure",
            ),
            (
                "CCC5014",
                "`fake-objcopy` failed during exact generated-symbol localization with exit status: 1: injected failure",
            ),
        ];
        for (fail_at, (expected_code, expected_message)) in expected.iter().enumerate() {
            let directory = test_directory(&format!("failure-{fail_at}"));
            let output = directory.join("result.o");
            fs::write(&output, b"old output").unwrap();
            let helper = "__ccc_call_helper_failure";
            let bundle = ArtifactBundle::new(
                make_object(&[(helper, false, SymbolScope::Unknown)]),
                vec![render_generic_call_helper(helper).unwrap()],
                BridgeManifestV1::new(
                    [2; 32],
                    vec![GeneratedSymbol::internal(
                        helper,
                        GeneratedSymbolKind::CallHelper,
                        GeneratedSymbolOwner::AssemblyUnit("call-helper".to_owned()),
                    )],
                ),
            );
            let error = package_artifact_bundle_with_runner(
                bundle,
                &output,
                &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
                &toolchain(),
                &FakeRunner::failing_at(fail_at),
            )
            .unwrap_err();
            assert_eq!(error.code, *expected_code, "phase {fail_at}");
            assert_eq!(error.message, *expected_message, "phase {fail_at}");
            assert_eq!(fs::read(&output).unwrap(), b"old output", "phase {fail_at}");
            assert_eq!(
                fs::read_dir(&directory).unwrap().count(),
                1,
                "phase {fail_at}"
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn missing_packaging_tools_have_stable_phase_specific_diagnostics() {
        let helper = "__ccc_call_helper_missing_tool";
        let make_bundle = || {
            ArtifactBundle::new(
                make_object(&[(helper, false, SymbolScope::Unknown)]),
                vec![render_generic_call_helper(helper).unwrap()],
                BridgeManifestV1::new(
                    [3; 32],
                    vec![GeneratedSymbol::internal(
                        helper,
                        GeneratedSymbolKind::CallHelper,
                        GeneratedSymbolOwner::AssemblyUnit("call-helper".to_owned()),
                    )],
                ),
            )
        };

        let directory = test_directory("missing-tool-configurations");
        let output = directory.join("result.o");
        let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();

        let error = package_artifact_bundle_with_runner(
            make_bundle(),
            &output,
            &config,
            &ToolchainSpec {
                object_copier: Some(ToolCommandSpec::new("fake-objcopy")),
                ..ToolchainSpec::default()
            },
            &FakeRunner::successful(),
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC5011");
        assert_eq!(
            error.message,
            "resolved toolchain for target `x86_64-unknown-linux-gnu` has no compiler driver for generated assembly"
        );

        let error = package_artifact_bundle_with_runner(
            make_bundle(),
            &output,
            &config,
            &ToolchainSpec {
                compiler_driver: Some(ToolCommandSpec::new("fake-cc")),
                ..ToolchainSpec::default()
            },
            &FakeRunner::successful(),
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC5011");
        assert_eq!(
            error.message,
            "resolved toolchain has no object copier for generated assembly"
        );

        let error = package_artifact_bundle_with_runner(
            make_bundle(),
            &output,
            &config,
            &ToolchainSpec {
                compiler_driver: Some(ToolCommandSpec::new("missing-cc")),
                object_copier: Some(ToolCommandSpec::new("fake-objcopy")),
                ..ToolchainSpec::default()
            },
            &MissingToolRunner {
                missing_program: "missing-cc",
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC5013");
        assert_eq!(
            error.message,
            "cannot invoke `missing-cc` for packaging capability primary: injected missing executable"
        );

        let error = package_artifact_bundle_with_runner(
            make_bundle(),
            &output,
            &config,
            &ToolchainSpec {
                compiler_driver: Some(ToolCommandSpec::new("fake-cc")),
                object_copier: Some(ToolCommandSpec::new("missing-objcopy")),
                ..ToolchainSpec::default()
            },
            &MissingToolRunner {
                missing_program: "missing-objcopy",
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC5012");
        assert_eq!(
            error.message,
            "no object copier supports exact x86-64 ELF symbol localization: missing-objcopy: cannot invoke `missing-objcopy` for exact generated-symbol localization: injected missing executable"
        );

        assert!(!output.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn source_internal_entry_resolves_before_exact_localization() {
        let directory = test_directory("source-internal");
        let output = directory.join("result.o");
        let public_symbol = "local_variadic";
        let hidden_body = "__ccc_variadic_body_local_test";
        let assembly = crate::bridge::render_variadic_entry(&crate::bridge::VariadicEntryPlan {
            public_symbol: public_symbol.to_owned(),
            hidden_body_symbol: hidden_body.to_owned(),
            linkage: crate::bridge::AssemblyFunctionLinkage::Internal,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 0,
            xmm_results: 0,
            hidden_return: false,
            logical_line: 1,
        })
        .unwrap();
        let primary = make_object_with_call_and_address_references(public_symbol, hidden_body);
        let parsed_primary = object::File::parse(primary.as_slice()).unwrap();
        assert_eq!(
            parsed_primary
                .sections()
                .flat_map(|section| section.relocations())
                .count(),
            2
        );
        let bundle = ArtifactBundle::new(
            primary,
            vec![assembly],
            BridgeManifestV1::new(
                [4; 32],
                vec![
                    GeneratedSymbol::source_internal(
                        public_symbol,
                        GeneratedSymbolKind::VariadicEntry,
                        GeneratedSymbolOwner::AssemblyUnit(
                            "variadic-entry-local_variadic".to_owned(),
                        ),
                    ),
                    GeneratedSymbol::internal(
                        hidden_body,
                        GeneratedSymbolKind::VariadicBody,
                        GeneratedSymbolOwner::PrimaryObject,
                    ),
                ],
            ),
        );
        package_artifact_bundle_with_runner(
            bundle,
            &output,
            &EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
            &toolchain(),
            &FakeRunner::successful(),
        )
        .unwrap();
        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbol = object
            .symbols()
            .find(|symbol| symbol.name() == Ok(public_symbol))
            .unwrap();
        assert_eq!(symbol.scope(), SymbolScope::Compilation);
        fs::remove_dir_all(directory).unwrap();
    }
}
