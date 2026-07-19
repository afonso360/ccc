//! Target tool resolution and executable link-plan execution.

pub mod artifact;
pub mod bridge;
mod package;
mod temp_cleanup;

pub use artifact::{
    ArtifactBundle, BridgeManifestV2, GeneratedSymbol, GeneratedSymbolBinding,
    GeneratedSymbolOwner, GeneratedSymbolVisibility, VerifiedArtifactBundle,
};
pub use package::{
    PackagingReport, PackagingToolIdentity, package_artifact_bundle,
    package_artifact_bundle_with_runner,
};
#[doc(hidden)]
pub use temp_cleanup::RegisteredTemporaryFile;

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
    RuntimeHelperProvider, SystemIncludeEntry, SystemIncludeKind, ToolCommandSpec,
    ToolchainFingerprint, ToolchainSpec, Triple,
};
use object::read::archive::ArchiveFile;
use object::read::{Object as _, ObjectSymbol as _};

/// Observable provider and command-line contribution for target runtime
/// helpers. The helper symbols themselves remain defined by the target
/// manifest; this plan records how the final link resolves them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeHelperLinkPlan {
    pub provider: RuntimeHelperProvider,
    pub symbols: Vec<&'static str>,
}

pub fn runtime_helper_link_plan(
    config: &EffectiveCompilationConfig,
) -> Option<RuntimeHelperLinkPlan> {
    let manifest = config.target.abi.runtime_helper_manifest();
    let provider = manifest.first()?.provider;
    debug_assert!(manifest.iter().all(|entry| entry.provider == provider));
    Some(RuntimeHelperLinkPlan {
        provider,
        symbols: manifest.iter().map(|entry| entry.symbol).collect(),
    })
}

#[derive(Clone, Default, Eq, PartialEq)]
struct LinkSymbolState {
    defined: HashSet<String>,
    weak_defined: HashSet<String>,
    common: HashSet<String>,
    dynamic_defined: HashSet<String>,
    unresolved: HashSet<String>,
    force_all_runtime_helpers: bool,
}

#[derive(Default)]
struct ObjectSymbolFacts {
    defined: HashSet<String>,
    weak_defined: HashSet<String>,
    common: HashSet<String>,
    undefined: HashSet<String>,
}

#[derive(Clone)]
struct ScannableLinkInput {
    path: PathBuf,
    whole_archive: bool,
    as_needed: bool,
    explicit: bool,
}

fn runtime_helper_link_plan_for_inputs(
    inputs: &[OsString],
    driver: &ToolCommandSpec,
    config: &EffectiveCompilationConfig,
) -> Result<Option<RuntimeHelperLinkPlan>, LinkError> {
    let manifest = config.target.abi.runtime_helper_manifest();
    if manifest.is_empty() {
        return Ok(None);
    }
    let ordered_arguments = driver
        .arguments
        .iter()
        .cloned()
        .chain(inputs.iter().cloned())
        .collect::<Vec<_>>();
    let driver_static = compiler_driver_static_mode(&ordered_arguments);
    let scan_inputs = expanded_linker_scan_arguments(&ordered_arguments);
    let inputs = scan_inputs.as_slice();
    let mut state = LinkSymbolState::default();
    state.unresolved.extend(forced_undefined_symbols(inputs));
    // GNU-compatible drivers apply every -L option to every -l option,
    // regardless of their relative command-line order.
    let search_directories =
        library_search_directories(inputs, config.toolchain.sysroot.as_deref());
    let mut static_libraries = driver_static;
    let mut whole_archive = false;
    let mut as_needed = false;
    let mut linker_state_stack = Vec::<(bool, bool, bool)>::new();
    let mut group_stack = Vec::<Vec<ScannableLinkInput>>::new();
    let mut index = 0;
    while index < inputs.len() {
        let argument = inputs[index].to_string_lossy();
        if matches!(argument.as_ref(), "--start-group" | "-start-group" | "-(") {
            group_stack.push(Vec::new());
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "--push-state" | "-push-state") {
            linker_state_stack.push((static_libraries, whole_archive, as_needed));
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "--pop-state" | "-pop-state") {
            if let Some((saved_static, saved_whole, saved_as_needed)) = linker_state_stack.pop() {
                static_libraries = saved_static;
                whole_archive = saved_whole;
                as_needed = saved_as_needed;
            }
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "--end-group" | "-end-group" | "-)") {
            if let Some(group) = group_stack.pop() {
                replay_link_group(&group, &mut state)?;
            }
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "-L" | "--library-path") {
            index += 2;
            continue;
        } else if argument.starts_with("-L") || argument.starts_with("--library-path=") {
            index += 1;
            continue;
        } else if matches!(
            argument.as_ref(),
            "--ccc-linker-static" | "--ccc-linker-Bstatic" | "-dn" | "-non_shared" | "-aarchive"
        ) {
            static_libraries = true;
            index += 1;
            continue;
        } else if matches!(
            argument.as_ref(),
            "--ccc-linker-Bdynamic" | "-dy" | "-call_shared" | "-ashared" | "-adefault"
        ) {
            static_libraries = false;
            index += 1;
            continue;
        } else if argument == "-static" {
            // Compiler drivers place their global static-link mode before
            // libraries even when the spelling occurs later in argv. The
            // initial state was precomputed above; positional linker-state
            // changes still apply after this point.
            index += 1;
            continue;
        } else if argument == "-a" {
            if let Some(mode) = inputs.get(index + 1).map(|value| value.to_string_lossy()) {
                match mode.as_ref() {
                    "archive" => static_libraries = true,
                    "shared" | "default" => static_libraries = false,
                    _ => state.force_all_runtime_helpers = true,
                }
            }
            index += 2;
            continue;
        } else if matches!(
            argument.as_ref(),
            "--whole-archive" | "-whole-archive" | "-Wl,--whole-archive"
        ) {
            whole_archive = true;
            index += 1;
            continue;
        } else if matches!(
            argument.as_ref(),
            "--no-whole-archive" | "-no-whole-archive" | "-Wl,--no-whole-archive"
        ) {
            whole_archive = false;
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "--as-needed" | "-as-needed") {
            as_needed = true;
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "--no-as-needed" | "-no-as-needed") {
            as_needed = false;
            index += 1;
            continue;
        } else if matches!(argument.as_ref(), "-nostartfiles" | "-nostdlib") {
            // The target linker's default script can name an entry symbol and
            // use that reference to extract an archive member. The entry is
            // target dependent, so omitting startup files makes selective
            // reconstruction unsafe.
            state.force_all_runtime_helpers = true;
            index += 1;
            continue;
        } else if let Some(width) = unmodeled_linker_argument_width(&argument) {
            state.force_all_runtime_helpers = true;
            index += width;
            continue;
        } else if linker_option_consumes_next(&argument) {
            index += 2;
            continue;
        }

        let library = if matches!(argument.as_ref(), "-l" | "--library") {
            let library = inputs
                .get(index + 1)
                .map(|value| value.to_string_lossy().into_owned());
            index += 2;
            library
        } else if let Some(library) = argument
            .strip_prefix("-l")
            .or_else(|| argument.strip_prefix("--library="))
        {
            index += 1;
            (!library.is_empty()).then(|| library.to_owned())
        } else {
            None
        };
        if let Some(library) = library {
            if let Some(path) = resolve_library_for_scan(
                &library,
                static_libraries,
                &search_directories,
                driver,
                config,
            ) {
                scan_and_record_link_input(
                    ScannableLinkInput {
                        path,
                        whole_archive,
                        as_needed,
                        explicit: false,
                    },
                    &mut state,
                    &mut group_stack,
                )?;
            } else {
                // The real driver may resolve a target-specific search path
                // that its probes did not expose. Do not let an uninspected
                // library produce a false negative in helper selection.
                state.force_all_runtime_helpers = true;
            }
            continue;
        }

        index += 1;
        if argument.starts_with('-') {
            continue;
        }
        let path = Path::new(argument.as_ref());
        if path.is_file() || looks_like_binary_link_input(path) {
            scan_and_record_link_input(
                ScannableLinkInput {
                    path: path.to_owned(),
                    whole_archive,
                    as_needed,
                    explicit: true,
                },
                &mut state,
                &mut group_stack,
            )?;
        }
    }

    let required = manifest
        .iter()
        .filter(|entry| state.force_all_runtime_helpers || state.unresolved.contains(entry.symbol))
        .collect::<Vec<_>>();
    let Some(first) = required.first() else {
        return Ok(None);
    };
    let provider = first.provider;
    if required.iter().any(|entry| entry.provider != provider) {
        return Err(LinkError {
            code: "CCC5008",
            message: "runtime-helper requirements select multiple providers".to_owned(),
        });
    }
    Ok(Some(RuntimeHelperLinkPlan {
        provider,
        symbols: required.iter().map(|entry| entry.symbol).collect(),
    }))
}

fn expanded_linker_scan_arguments(inputs: &[OsString]) -> Vec<OsString> {
    let mut expanded = Vec::with_capacity(inputs.len());
    let mut index = 0;
    while index < inputs.len() {
        let argument = inputs[index].to_string_lossy();
        if argument == "-Xlinker" {
            if let Some(value) = inputs.get(index + 1) {
                expanded.push(normalize_linker_passthrough(value));
                index += 2;
            } else {
                expanded.push(inputs[index].clone());
                index += 1;
            }
            continue;
        }
        if let Some(arguments) = argument
            .strip_prefix("-Wl,")
            .or_else(|| argument.strip_prefix("-Wl="))
        {
            expanded.extend(
                arguments
                    .split(',')
                    .map(OsStr::new)
                    .map(normalize_linker_passthrough),
            );
            index += 1;
            continue;
        }
        expanded.push(inputs[index].clone());
        index += 1;
    }
    expanded
}

fn compiler_driver_static_mode(arguments: &[OsString]) -> bool {
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == OsStr::new("-Xlinker") {
            index += 2;
            continue;
        }
        if arguments[index] == OsStr::new("-static") {
            return true;
        }
        index += 1;
    }
    false
}

fn normalize_linker_passthrough(argument: &OsStr) -> OsString {
    match argument.to_str() {
        Some("-static") => OsString::from("--ccc-linker-static"),
        Some("-Bstatic") => OsString::from("--ccc-linker-Bstatic"),
        Some("-Bdynamic") => OsString::from("--ccc-linker-Bdynamic"),
        _ => argument.to_owned(),
    }
}

fn library_search_directories(inputs: &[OsString], sysroot: Option<&Path>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut index = 0;
    while index < inputs.len() {
        let argument = inputs[index].to_string_lossy();
        if matches!(argument.as_ref(), "-L" | "--library-path") {
            if let Some(directory) = inputs.get(index + 1) {
                directories.push(resolve_sysroot_path(&directory.to_string_lossy(), sysroot));
                index += 2;
                continue;
            }
        } else if let Some(directory) = argument
            .strip_prefix("-L")
            .or_else(|| argument.strip_prefix("--library-path="))
            && !directory.is_empty()
        {
            directories.push(resolve_sysroot_path(directory, sysroot));
        }
        index += 1;
    }
    directories
}

fn forced_undefined_symbols(inputs: &[OsString]) -> HashSet<String> {
    let mut symbols = HashSet::new();
    let mut index = 0;
    while index < inputs.len() {
        let argument = inputs[index].to_string_lossy();
        if matches!(
            argument.as_ref(),
            "-u" | "--undefined" | "-require-defined" | "--require-defined"
        ) {
            if let Some(symbol) = inputs.get(index + 1) {
                symbols.insert(symbol.to_string_lossy().into_owned());
                index += 2;
                continue;
            }
        } else if matches!(argument.as_ref(), "-e" | "--entry") {
            if let Some(symbol) = inputs.get(index + 1) {
                insert_symbolic_entry(&mut symbols, &symbol.to_string_lossy());
                index += 2;
                continue;
            }
        } else if let Some(symbol) = argument
            .strip_prefix("--undefined=")
            .or_else(|| argument.strip_prefix("--require-defined="))
            .or_else(|| argument.strip_prefix("-require-defined="))
        {
            if !symbol.is_empty() {
                symbols.insert(symbol.to_owned());
            }
        } else if let Some(symbol) = argument.strip_prefix("--entry=") {
            insert_symbolic_entry(&mut symbols, symbol);
        } else if !argument.starts_with("--") {
            if let Some(symbol) = argument.strip_prefix("-u")
                && !symbol.is_empty()
            {
                symbols.insert(symbol.to_owned());
            } else if let Some(symbol) = argument.strip_prefix("-e") {
                insert_symbolic_entry(&mut symbols, symbol);
            }
        }
        index += 1;
    }
    symbols
}

fn insert_symbolic_entry(symbols: &mut HashSet<String>, entry: &str) {
    let unsigned = entry.strip_prefix(['+', '-']).unwrap_or(entry);
    let numeric = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || (!unsigned.is_empty()
            && if unsigned.starts_with('0') {
                unsigned.bytes().all(|byte| matches!(byte, b'0'..=b'7'))
            } else {
                unsigned.bytes().all(|byte| byte.is_ascii_digit())
            });
    if !entry.is_empty() && !numeric {
        symbols.insert(entry.to_owned());
    }
}

fn linker_option_consumes_next(argument: &str) -> bool {
    matches!(
        argument,
        "-u" | "--undefined"
            | "-require-defined"
            | "--require-defined"
            | "-rpath"
            | "--rpath"
            | "-rpath-link"
            | "--rpath-link"
            | "-soname"
            | "--soname"
            | "-h"
            | "-Map"
            | "--Map"
            | "--version-script"
            | "--dynamic-list"
            | "--retain-symbols-file"
            | "-z"
            | "-m"
            | "-e"
            | "--entry"
            | "--dynamic-linker"
            | "-plugin"
            | "-plugin-opt"
            | "-framework"
            | "-install_name"
            | "-compatibility_version"
            | "-current_version"
            | "-undefined"
            | "-arch"
    )
}

fn unmodeled_linker_argument_width(argument: &str) -> Option<usize> {
    if matches!(
        argument,
        "-T" | "-dT"
            | "--script"
            | "--default-script"
            | "--just-symbols"
            | "-R"
            | "--defsym"
            | "--wrap"
    ) {
        return Some(2);
    }
    if argument.starts_with('@')
        || matches!(argument, "--start-lib" | "--end-lib")
        || (argument.starts_with("-T") && argument.len() > 2)
        || (argument.starts_with("-dT") && argument.len() > 3)
        || argument.starts_with("--script=")
        || argument.starts_with("--default-script=")
        || argument.starts_with("--just-symbols=")
        || argument.starts_with("--defsym=")
        || argument.starts_with("--wrap=")
    {
        return Some(1);
    }
    None
}

fn resolve_library_for_scan(
    library: &str,
    static_libraries: bool,
    search_directories: &[PathBuf],
    driver: &ToolCommandSpec,
    config: &EffectiveCompilationConfig,
) -> Option<PathBuf> {
    if let Some(file_name) = library.strip_prefix(':') {
        if let Some(path) = search_directories
            .iter()
            .map(|directory| directory.join(file_name))
            .find(|path| path.is_file())
        {
            return Some(path);
        }
        let output = tool_command(driver)
            .arg(format!("-print-file-name={file_name}"))
            .output()
            .ok()?;
        if output.status.success() {
            let reported = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if reported.as_os_str() != OsStr::new(file_name) && reported.is_file() {
                return Some(reported);
            }
        }
        return None;
    }
    let dynamic_suffix = if config.target.triple.binary_format == ccc_target::BinaryFormat::Macho {
        "dylib"
    } else {
        "so"
    };
    let suffixes: &[&str] = if static_libraries {
        &["a"]
    } else {
        &[dynamic_suffix, "a"]
    };
    for directory in search_directories {
        for suffix in suffixes {
            let path = directory.join(format!("lib{library}.{suffix}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    for directory in driver_library_search_directories(driver, config.toolchain.sysroot.as_deref())
    {
        for suffix in suffixes {
            let path = directory.join(format!("lib{library}.{suffix}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    // Retain the historical query as a fallback for drivers whose search-dir
    // report contains target-specific placeholders that the host cannot
    // resolve directly.
    for suffix in suffixes {
        let file_name = format!("lib{library}.{suffix}");
        let Ok(output) = tool_command(driver)
            .arg(format!("-print-file-name={file_name}"))
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let reported = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if reported.as_os_str() != OsStr::new(&file_name) && reported.is_file() {
            return Some(reported);
        }
    }
    None
}

fn driver_library_search_directories(
    driver: &ToolCommandSpec,
    sysroot: Option<&Path>,
) -> Vec<PathBuf> {
    let Ok(output) = tool_command(driver).arg("-print-search-dirs").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(value) = text.lines().find_map(|line| {
        line.strip_prefix("libraries:")
            .map(str::trim)
            .map(|value| value.strip_prefix('=').unwrap_or(value))
    }) else {
        return Vec::new();
    };
    env::split_paths(OsStr::new(value))
        .map(|path| {
            let text = path.to_string_lossy();
            resolve_sysroot_path(&text, sysroot)
        })
        .collect()
}

fn scan_and_record_link_input(
    input: ScannableLinkInput,
    state: &mut LinkSymbolState,
    groups: &mut [Vec<ScannableLinkInput>],
) -> Result<(), LinkError> {
    process_link_input(
        &input.path,
        state,
        input.whole_archive,
        input.as_needed,
        input.explicit,
    )?;
    for group in groups {
        group.push(input.clone());
    }
    Ok(())
}

fn replay_link_group(
    inputs: &[ScannableLinkInput],
    state: &mut LinkSymbolState,
) -> Result<(), LinkError> {
    loop {
        let before = state.clone();
        for input in inputs {
            process_link_input(
                &input.path,
                state,
                input.whole_archive,
                input.as_needed,
                input.explicit,
            )?;
        }
        if *state == before {
            return Ok(());
        }
    }
}

fn process_link_input(
    path: &Path,
    state: &mut LinkSymbolState,
    whole_archive: bool,
    as_needed: bool,
    explicit: bool,
) -> Result<(), LinkError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_error) if !explicit => return Ok(()),
        Err(error) => {
            return Err(LinkError {
                code: "CCC5008",
                message: format!(
                    "cannot inspect `{}` for runtime-helper requirements: {error}",
                    path.display()
                ),
            });
        }
    };
    if ArchiveFile::parse(bytes.as_slice()).is_ok() {
        return process_archive(path, &bytes, state, whole_archive);
    }
    let object = match object::File::parse(bytes.as_slice()) {
        Ok(object) => object,
        Err(_error) => {
            // GNU-compatible linkers accept augmenting linker scripts and
            // plugin/LTO inputs in the same positions as objects. Their
            // selection graphs remain the real linker's responsibility, so
            // helper selection is deliberately conservative.
            state.force_all_runtime_helpers = true;
            return Ok(());
        }
    };
    if object.kind() == object::ObjectKind::Dynamic {
        process_dynamic_object(&object, state, as_needed);
    } else {
        process_object_facts(state, &object_symbol_facts(&object));
    }
    Ok(())
}

fn looks_like_dynamic_library(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("so" | "dylib")
    ) || path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.contains(".so."))
}

fn looks_like_binary_link_input(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("o" | "lo" | "obj" | "a" | "so" | "dylib")
    ) || looks_like_dynamic_library(path)
}

fn process_dynamic_object(object: &object::File<'_>, state: &mut LinkSymbolState, as_needed: bool) {
    let definitions = object
        .dynamic_symbols()
        .filter(|symbol| {
            symbol.is_global()
                && symbol.scope() == object::SymbolScope::Dynamic
                && matches!(
                    symbol.section(),
                    object::SymbolSection::Section(_) | object::SymbolSection::Absolute
                )
        })
        .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
        .collect::<HashSet<_>>();
    apply_dynamic_definitions(state, definitions, as_needed);
}

fn apply_dynamic_definitions(
    state: &mut LinkSymbolState,
    definitions: HashSet<String>,
    as_needed: bool,
) {
    if as_needed
        && !definitions
            .iter()
            .any(|symbol| state.unresolved.contains(symbol))
    {
        return;
    }
    state
        .unresolved
        .retain(|symbol| !definitions.contains(symbol));
    state.dynamic_defined.extend(definitions);
}

fn process_archive(
    path: &Path,
    bytes: &[u8],
    state: &mut LinkSymbolState,
    whole_archive: bool,
) -> Result<(), LinkError> {
    let archive = ArchiveFile::parse(bytes).map_err(|error| LinkError {
        code: "CCC5008",
        message: format!(
            "cannot parse archive `{}` for runtime-helper requirements: {error}",
            path.display()
        ),
    })?;
    let mut members = Vec::new();
    for member in archive.members() {
        let member = member.map_err(|error| LinkError {
            code: "CCC5008",
            message: format!(
                "cannot inspect a member of `{}` for runtime-helper requirements: {error}",
                path.display()
            ),
        })?;
        let data = if member.is_thin() {
            let member_path = path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(archive_member_path(member.name()));
            Cow::Owned(fs::read(&member_path).map_err(|error| LinkError {
                code: "CCC5008",
                message: format!(
                    "cannot read thin archive member `{}` from `{}`: {error}",
                    String::from_utf8_lossy(member.name()),
                    member_path.display()
                ),
            })?)
        } else {
            Cow::Borrowed(member.data(bytes).map_err(|error| LinkError {
                code: "CCC5008",
                message: format!(
                    "cannot read archive member `{}` in `{}`: {error}",
                    String::from_utf8_lossy(member.name()),
                    path.display()
                ),
            })?)
        };
        if let Ok(object) = object::File::parse(data.as_ref()) {
            members.push(Some(object_symbol_facts(&object)));
        } else {
            // An ordinary unparseable member may be linker-plugin input such
            // as LLVM bitcode. Its extraction cannot be simulated safely.
            state.force_all_runtime_helpers = true;
        }
    }
    if whole_archive {
        for member in members.into_iter().flatten() {
            process_object_facts(state, &member);
        }
        return Ok(());
    }

    loop {
        let mut selected = false;
        for member in &mut members {
            let Some(facts) = member else {
                continue;
            };
            if facts
                .defined
                .iter()
                .chain(&facts.weak_defined)
                .any(|symbol| state.unresolved.contains(symbol) || state.common.contains(symbol))
                || facts
                    .common
                    .iter()
                    .any(|symbol| state.unresolved.contains(symbol))
            {
                let facts = member.take().expect("selected archive member is present");
                process_object_facts(state, &facts);
                selected = true;
            }
        }
        if !selected {
            break;
        }
    }
    Ok(())
}

fn archive_member_path(name: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(OsStr::from_bytes(name))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(name).into_owned())
    }
}

fn object_symbol_facts(object: &object::File<'_>) -> ObjectSymbolFacts {
    let mut facts = ObjectSymbolFacts::default();
    for symbol in object.symbols().filter(|symbol| symbol.is_global()) {
        let Ok(name) = symbol.name() else {
            continue;
        };
        if matches!(symbol.section(), object::SymbolSection::Common) {
            facts.common.insert(name.to_owned());
        } else if matches!(
            symbol.section(),
            object::SymbolSection::Section(_) | object::SymbolSection::Absolute
        ) {
            if symbol.is_weak() {
                facts.weak_defined.insert(name.to_owned());
            } else {
                facts.defined.insert(name.to_owned());
            }
        } else if matches!(symbol.section(), object::SymbolSection::Undefined) && !symbol.is_weak()
        {
            facts.undefined.insert(name.to_owned());
        }
    }
    facts
}

fn process_object_facts(state: &mut LinkSymbolState, facts: &ObjectSymbolFacts) {
    state
        .unresolved
        .retain(|symbol| !facts.defined.contains(symbol));
    state
        .common
        .retain(|symbol| !facts.defined.contains(symbol));
    state
        .weak_defined
        .retain(|symbol| !facts.defined.contains(symbol));
    state.defined.extend(facts.defined.iter().cloned());
    for common in &facts.common {
        if !state.defined.contains(common) {
            state.unresolved.remove(common);
            state.weak_defined.remove(common);
            state.common.insert(common.clone());
        }
    }
    for weak in &facts.weak_defined {
        if !state.defined.contains(weak) && !state.common.contains(weak) {
            state.unresolved.remove(weak);
            state.weak_defined.insert(weak.clone());
        }
    }
    state.unresolved.extend(
        facts
            .undefined
            .iter()
            .filter(|symbol| {
                !state.defined.contains(*symbol)
                    && !state.common.contains(*symbol)
                    && !state.weak_defined.contains(*symbol)
                    && !state.dynamic_defined.contains(*symbol)
            })
            .cloned(),
    );
}

fn resolve_runtime_helper_provider(
    driver: &ToolCommandSpec,
    plan: &RuntimeHelperLinkPlan,
) -> Result<PathBuf, LinkError> {
    let RuntimeHelperProvider::CompilerBuiltins = plan.provider;
    // Both GCC and Clang implement this historical query. Depending on the
    // driver's configured runtime, its result can be libgcc or compiler-rt.
    let output = tool_command(driver)
        .arg("-print-libgcc-file-name")
        .output()
        .map_err(|error| LinkError {
            code: "CCC5008",
            message: format!(
                "cannot query runtime-helper provider from `{}`: {error}",
                driver.display()
            ),
        })?;
    if !output.status.success() {
        return Err(LinkError {
            code: "CCC5008",
            message: format!(
                "target compiler driver `{}` cannot resolve its compiler builtins archive: {}",
                driver.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let reported = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(reported.trim());
    if path.as_os_str().is_empty() {
        return Err(LinkError {
            code: "CCC5008",
            message: format!(
                "target compiler driver `{}` reported an empty compiler builtins path",
                driver.display()
            ),
        });
    }
    verify_runtime_helper_archive(&path, plan)?;
    fs::canonicalize(&path).map_err(|error| LinkError {
        code: "CCC5008",
        message: format!(
            "resolved compiler builtins provider `{}` cannot be canonicalized: {error}",
            path.display()
        ),
    })
}

fn verify_runtime_helper_archive(
    path: &Path,
    plan: &RuntimeHelperLinkPlan,
) -> Result<(), LinkError> {
    let bytes = fs::read(path).map_err(|error| LinkError {
        code: "CCC5008",
        message: format!(
            "resolved compiler builtins provider `{}` cannot be read: {error}",
            path.display()
        ),
    })?;
    let archive = ArchiveFile::parse(bytes.as_slice()).map_err(|error| LinkError {
        code: "CCC5008",
        message: format!(
            "resolved compiler builtins provider `{}` is not a readable archive: {error}",
            path.display()
        ),
    })?;
    let symbols = archive
        .symbols()
        .map_err(|error| LinkError {
            code: "CCC5008",
            message: format!(
                "cannot read symbols from compiler builtins provider `{}`: {error}",
                path.display()
            ),
        })?
        .ok_or_else(|| LinkError {
            code: "CCC5008",
            message: format!(
                "compiler builtins provider `{}` has no archive symbol index",
                path.display()
            ),
        })?
        .map(|symbol| symbol.map(|symbol| String::from_utf8_lossy(symbol.name()).into_owned()))
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| LinkError {
            code: "CCC5008",
            message: format!(
                "cannot read symbols from compiler builtins provider `{}`: {error}",
                path.display()
            ),
        })?;
    verify_runtime_helper_symbols(path, &symbols, plan)
}

fn verify_runtime_helper_symbols(
    path: &Path,
    symbols: &HashSet<String>,
    plan: &RuntimeHelperLinkPlan,
) -> Result<(), LinkError> {
    let missing = plan
        .symbols
        .iter()
        .copied()
        .filter(|symbol| !symbols.contains(*symbol))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(LinkError {
            code: "CCC5008",
            message: format!(
                "compiler builtins provider `{}` is missing required runtime helpers: {}",
                path.display(),
                missing.join(", ")
            ),
        });
    }
    Ok(())
}

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

    /// Mach-O partial linking preserves source-hidden private externs, then a
    /// Mach-native symbol editor localizes only compiler-internal manifest
    /// symbols while updating symbol-indexed relocations.
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
    "CCC_NMEDIT",
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
            target_arguments.push(OsString::from(format!("-mmacosx-version-min={version}")));
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
                    let Ok(fresh_candidate) = self.driver.clone().map_or_else(
                        || driver_from_environment(&fresh_environment, &self.target),
                        Ok,
                    ) else {
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutputKind {
    Executable,
    Shared,
    Relocatable,
}

fn append_runtime_helper_providers(
    command: &mut Command,
    providers: &BTreeSet<PathBuf>,
    format: ccc_target::BinaryFormat,
) {
    if providers.is_empty() {
        return;
    }
    if format == ccc_target::BinaryFormat::Elf {
        command.arg("-Wl,--push-state,--no-whole-archive");
        command.args(providers);
        command.arg("-Wl,--pop-state");
    } else {
        command.args(providers);
    }
}

/// Link an ordered mixture of objects, archives, libraries, and driver
/// arguments with a previously resolved target toolchain.
pub fn link_inputs_with_toolchain(
    inputs: &[OsString],
    output: &Path,
    kind: LinkOutputKind,
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
) -> Result<(), LinkError> {
    config
        .validate_target_profile_options()
        .map_err(|message| LinkError {
            code: "CCC5005",
            message,
        })?;
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
    command.args(inputs).arg("-o").arg(output);

    let mut providers = BTreeSet::new();
    if let Some(runtime_helpers) = runtime_helper_link_plan_for_inputs(inputs, driver, config)? {
        providers.insert(resolve_runtime_helper_provider(driver, &runtime_helpers)?);
    }
    // Helper providers follow user objects and archives so their members
    // participate in normal left-to-right extraction. Isolate them from a
    // user --whole-archive state: loading an entire compiler runtime is both
    // semantically wrong and likely to create duplicate definitions.
    append_runtime_helper_providers(&mut command, &providers, config.target.triple.binary_format);
    match kind {
        LinkOutputKind::Executable => {
            command.arg(relocation_link_argument(config));
        }
        LinkOutputKind::Shared => {
            command.arg(
                if config.target.triple.binary_format == ccc_target::BinaryFormat::Macho {
                    "-dynamiclib"
                } else {
                    "-shared"
                },
            );
        }
        LinkOutputKind::Relocatable => {
            command.arg("-r");
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

/// Link with a previously resolved toolchain without executing discovery probes.
pub fn link_executable_with_toolchain(
    object: &Path,
    output: &Path,
    config: &EffectiveCompilationConfig,
    toolchain: &ToolchainSpec,
) -> Result<(), LinkError> {
    link_inputs_with_toolchain(
        &[object.as_os_str().to_owned()],
        output,
        LinkOutputKind::Executable,
        config,
        toolchain,
    )
}

fn relocation_link_argument(config: &EffectiveCompilationConfig) -> &'static str {
    match (config.target.triple.binary_format, config.relocation_model) {
        (ccc_target::BinaryFormat::Macho, RelocationModel::Static) => "-Wl,-no_pie",
        (ccc_target::BinaryFormat::Macho, RelocationModel::Pic | RelocationModel::Pie) => {
            "-Wl,-pie"
        }
        (_, RelocationModel::Static) => "-no-pie",
        (_, RelocationModel::Pic | RelocationModel::Pie) => "-pie",
    }
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
        .map(OsStr::to_os_string)
        .map_or_else(
            || {
                Ok(match target.architecture {
                    Architecture::Aarch64(_)
                        if target.operating_system == OperatingSystem::Linux =>
                    {
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

    use object::write::{Object, Symbol, SymbolSection};
    use object::{Architecture as ObjectArchitecture, BinaryFormat, Endianness, SymbolFlags};

    use super::*;

    #[derive(Clone, Copy)]
    enum FixtureSymbol {
        Strong,
        Weak,
        Common,
        Undefined,
        WeakUndefined,
        ZeroSizedNotype,
    }

    fn fixture_object(symbols: &[(&str, FixtureSymbol)]) -> Vec<u8> {
        let mut object = Object::new(
            BinaryFormat::Elf,
            ObjectArchitecture::X86_64,
            Endianness::Little,
        );
        let text = object.section_id(object::write::StandardSection::Text);
        object.append_section_data(text, &[0xc3], 1);
        for (name, fixture) in symbols {
            let (value, size, kind, scope, weak, section) = match fixture {
                FixtureSymbol::Strong => (
                    0,
                    1,
                    object::SymbolKind::Text,
                    object::SymbolScope::Linkage,
                    false,
                    SymbolSection::Section(text),
                ),
                FixtureSymbol::Weak => (
                    0,
                    1,
                    object::SymbolKind::Text,
                    object::SymbolScope::Linkage,
                    true,
                    SymbolSection::Section(text),
                ),
                FixtureSymbol::Common => (
                    8,
                    8,
                    object::SymbolKind::Data,
                    object::SymbolScope::Linkage,
                    false,
                    SymbolSection::Common,
                ),
                FixtureSymbol::Undefined | FixtureSymbol::WeakUndefined => (
                    0,
                    0,
                    object::SymbolKind::Unknown,
                    object::SymbolScope::Unknown,
                    matches!(fixture, FixtureSymbol::WeakUndefined),
                    SymbolSection::Undefined,
                ),
                FixtureSymbol::ZeroSizedNotype => (
                    0,
                    0,
                    object::SymbolKind::Label,
                    object::SymbolScope::Linkage,
                    false,
                    SymbolSection::Section(text),
                ),
            };
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size,
                kind,
                scope,
                weak,
                section,
                flags: SymbolFlags::None,
            });
        }
        object.write().unwrap()
    }

    fn symbol_object(defined: &[&str], undefined: &[&str]) -> Vec<u8> {
        let symbols = defined
            .iter()
            .map(|name| (*name, FixtureSymbol::Strong))
            .chain(
                undefined
                    .iter()
                    .map(|name| (*name, FixtureSymbol::Undefined)),
            )
            .collect::<Vec<_>>();
        fixture_object(&symbols)
    }

    fn archive(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut archive = b"!<arch>\n".to_vec();
        for (name, data) in members {
            let name = format!("{name}/");
            let header = format!(
                "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                0,
                0,
                0,
                "100644",
                data.len()
            );
            assert_eq!(header.len(), 60);
            archive.extend_from_slice(header.as_bytes());
            archive.extend_from_slice(data);
            if data.len() % 2 != 0 {
                archive.push(b'\n');
            }
        }
        archive
    }

    fn thin_archive(member_name: &str, member_size: usize) -> Vec<u8> {
        assert!(member_name.len() < 16);
        let mut archive = b"!<thin>\n".to_vec();
        let name = format!("{member_name}/");
        let header = format!(
            "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            0, 0, 0, "100644", member_size
        );
        assert_eq!(header.len(), 60);
        archive.extend_from_slice(header.as_bytes());
        archive
    }

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
    fn executable_relocation_flags_follow_the_target_driver() {
        let mut linux = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        assert_eq!(relocation_link_argument(&linux), "-pie");
        linux.relocation_model = RelocationModel::Static;
        assert_eq!(relocation_link_argument(&linux), "-no-pie");

        let darwin = EffectiveCompilationConfig::aarch64_apple_darwin();
        assert_eq!(relocation_link_argument(&darwin), "-Wl,-pie");
        let mut unsupported = darwin;
        unsupported.relocation_model = RelocationModel::Static;
        assert!(unsupported.validate_target_profile_options().is_err());
    }

    #[test]
    fn runtime_helper_link_plan_names_the_provider_and_symbols() {
        let linux = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        let plan = runtime_helper_link_plan(&linux).unwrap();
        assert_eq!(plan.provider, RuntimeHelperProvider::CompilerBuiltins);
        assert_eq!(plan.symbols.len(), 16);
        assert!(plan.symbols.contains(&"__divti3"));
        assert!(plan.symbols.contains(&"__fixunsdfti"));
        assert!(plan.symbols.contains(&"__floattixf"));
        assert!(plan.symbols.contains(&"__fixunsxfti"));

        let darwin = EffectiveCompilationConfig::aarch64_apple_darwin();
        assert_eq!(runtime_helper_link_plan(&darwin), None);
    }

    #[test]
    fn runtime_helper_scan_follows_archive_extraction_and_library_search() {
        let directory = env::temp_dir().join(format!(
            "ccc-runtime-helper-scan-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let main = directory.join("main.o");
        fs::write(&main, symbol_object(&[], &["needed"])).unwrap();
        let library = directory.join("libselected.a");
        let selected_without_helper = symbol_object(&["needed"], &[]);
        let unselected_with_helper = symbol_object(&["unused"], &["__divti3"]);
        fs::write(
            &library,
            archive(&[
                ("selected.o", selected_without_helper),
                ("unselected.o", unselected_with_helper),
            ]),
        )
        .unwrap();

        let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        let driver = ToolCommandSpec::new("/definitely/not/invoked");
        let explicit = [main.as_os_str().to_owned(), library.as_os_str().to_owned()];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&explicit, &driver, &config).unwrap(),
            None,
            "an unselected archive member must not pull in its helper"
        );
        let whole_archive = [
            main.as_os_str().to_owned(),
            OsString::from("--whole-archive"),
            library.as_os_str().to_owned(),
            OsString::from("--no-whole-archive"),
        ];
        let whole_plan = runtime_helper_link_plan_for_inputs(&whole_archive, &driver, &config)
            .unwrap()
            .unwrap();
        assert_eq!(whole_plan.symbols, ["__divti3"]);

        fs::write(
            &library,
            archive(&[("selected.o", symbol_object(&["needed"], &["__divti3"]))]),
        )
        .unwrap();
        let searched = [
            main.as_os_str().to_owned(),
            OsString::from("-L"),
            directory.as_os_str().to_owned(),
            OsString::from("-lselected"),
        ];
        let plan = runtime_helper_link_plan_for_inputs(&searched, &driver, &config)
            .unwrap()
            .unwrap();
        assert_eq!(plan.symbols, ["__divti3"]);
        let later_search_path = [
            main.as_os_str().to_owned(),
            OsString::from("-lselected"),
            OsString::from("-L"),
            directory.as_os_str().to_owned(),
        ];
        let later_plan = runtime_helper_link_plan_for_inputs(&later_search_path, &driver, &config)
            .unwrap()
            .unwrap();
        assert_eq!(later_plan.symbols, ["__divti3"]);
        let long_library_options = [
            main.as_os_str().to_owned(),
            OsString::from("--library=selected"),
            OsString::from(format!("--library-path={}", directory.display())),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&long_library_options, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols,
            ["__divti3"]
        );
        let exact_name = [
            main.as_os_str().to_owned(),
            OsString::from(format!("-L{}", directory.display())),
            OsString::from("-l:libselected.a"),
        ];
        let exact_plan = runtime_helper_link_plan_for_inputs(&exact_name, &driver, &config)
            .unwrap()
            .unwrap();
        assert_eq!(exact_plan.symbols, ["__divti3"]);

        let first = directory.join("first");
        let second = directory.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(
            first.join("libpriority.a"),
            archive(&[("selected.o", symbol_object(&["needed"], &["__divti3"]))]),
        )
        .unwrap();
        fs::write(
            second.join("libpriority.so"),
            symbol_object(&["needed"], &[]),
        )
        .unwrap();
        let directory_priority = [
            main.as_os_str().to_owned(),
            OsString::from("-L"),
            first.as_os_str().to_owned(),
            OsString::from("-L"),
            second.as_os_str().to_owned(),
            OsString::from("-lpriority"),
        ];
        let priority_plan =
            runtime_helper_link_plan_for_inputs(&directory_priority, &driver, &config)
                .unwrap()
                .unwrap();
        assert_eq!(priority_plan.symbols, ["__divti3"]);

        let wrong_order = [library.as_os_str().to_owned(), main.as_os_str().to_owned()];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&wrong_order, &driver, &config).unwrap(),
            None,
            "a non-group archive is not revisited for a later undefined symbol"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_helper_scan_matches_archive_group_and_symbol_precedence_rules() {
        let directory = env::temp_dir().join(format!(
            "ccc-runtime-helper-symbol-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        let driver = ToolCommandSpec::new("/definitely/not/invoked");

        let group_main = directory.join("group-main.o");
        let group_a = directory.join("group-a.a");
        let group_b = directory.join("group-b.a");
        fs::write(&group_main, symbol_object(&[], &["from_b"])).unwrap();
        fs::write(
            &group_a,
            archive(&[("a.o", symbol_object(&["from_a"], &["__divti3"]))]),
        )
        .unwrap();
        fs::write(
            &group_b,
            archive(&[("b.o", symbol_object(&["from_b"], &["from_a"]))]),
        )
        .unwrap();
        let without_group = [
            group_main.as_os_str().to_owned(),
            group_a.as_os_str().to_owned(),
            group_b.as_os_str().to_owned(),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&without_group, &driver, &config).unwrap(),
            None
        );
        let with_group = [
            group_main.as_os_str().to_owned(),
            OsString::from("-Wl,--start-group"),
            group_a.as_os_str().to_owned(),
            group_b.as_os_str().to_owned(),
            OsString::from("-Wl,--end-group"),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&with_group, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols,
            ["__divti3"]
        );

        let forced = directory.join("forced.a");
        fs::write(
            &forced,
            archive(&[("forced.o", symbol_object(&["forced_entry"], &["__divti3"]))]),
        )
        .unwrap();
        for inputs in [
            vec![
                OsString::from("-u"),
                OsString::from("forced_entry"),
                forced.as_os_str().to_owned(),
            ],
            vec![
                forced.as_os_str().to_owned(),
                OsString::from("--undefined=forced_entry"),
            ],
        ] {
            assert_eq!(
                runtime_helper_link_plan_for_inputs(&inputs, &driver, &config)
                    .unwrap()
                    .unwrap()
                    .symbols,
                ["__divti3"]
            );
        }
        let fixed_argument_driver =
            ToolCommandSpec::with_arguments("/definitely/not/invoked", ["-Wl,-u,forced_entry"]);
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[forced.as_os_str().to_owned()],
                &fixed_argument_driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"],
            "fixed target-driver arguments participate before user inputs"
        );

        for inputs in [
            vec![
                OsString::from("-eforced_entry"),
                forced.as_os_str().to_owned(),
            ],
            vec![
                forced.as_os_str().to_owned(),
                OsString::from("--entry=forced_entry"),
            ],
            vec![
                OsString::from("-e"),
                OsString::from("forced_entry"),
                forced.as_os_str().to_owned(),
            ],
            vec![
                forced.as_os_str().to_owned(),
                OsString::from("--entry"),
                OsString::from("forced_entry"),
            ],
        ] {
            assert_eq!(
                runtime_helper_link_plan_for_inputs(&inputs, &driver, &config)
                    .unwrap()
                    .unwrap()
                    .symbols,
                ["__divti3"]
            );
        }
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    OsString::from("-e"),
                    OsString::from("0x1000"),
                    forced.as_os_str().to_owned()
                ],
                &driver,
                &config,
            )
            .unwrap(),
            None,
            "a numeric entry address must not extract an archive member"
        );
        let digit_symbol = directory.join("digit-symbol.a");
        fs::write(
            &digit_symbol,
            archive(&[("digit.o", symbol_object(&["123abc", "08"], &["__divti3"]))]),
        )
        .unwrap();
        for entry in ["-e123abc", "-e08"] {
            assert_eq!(
                runtime_helper_link_plan_for_inputs(
                    &[OsString::from(entry), digit_symbol.as_os_str().to_owned(),],
                    &driver,
                    &config,
                )
                .unwrap()
                .unwrap()
                .symbols,
                ["__divti3"],
                "a partially numeric entry remains a symbol"
            );
        }

        let default_entry = directory.join("default-entry.a");
        fs::write(
            &default_entry,
            archive(&[("start.o", symbol_object(&["_start"], &["__divti3"]))]),
        )
        .unwrap();
        for suppression in ["-nostartfiles", "-nostdlib"] {
            let inputs = [
                OsString::from(suppression),
                default_entry.as_os_str().to_owned(),
            ];
            let plan = runtime_helper_link_plan_for_inputs(&inputs, &driver, &config)
                .unwrap()
                .unwrap();
            assert_eq!(
                plan.symbols.len(),
                config.target.abi.runtime_helper_manifest().len(),
                "the target-dependent default entry must use the conservative provider plan"
            );
        }

        let common_main = directory.join("common-main.o");
        let weak_archive = directory.join("weak.a");
        let strong_archive = directory.join("strong.a");
        fs::write(
            &common_main,
            fixture_object(&[("replaceable", FixtureSymbol::Common)]),
        )
        .unwrap();
        fs::write(
            &weak_archive,
            archive(&[(
                "weak.o",
                fixture_object(&[("replaceable", FixtureSymbol::Weak)]),
            )]),
        )
        .unwrap();
        fs::write(
            &strong_archive,
            archive(&[("strong.o", symbol_object(&["replaceable"], &["__divti3"]))]),
        )
        .unwrap();
        let common_chain = [
            common_main.as_os_str().to_owned(),
            weak_archive.as_os_str().to_owned(),
            strong_archive.as_os_str().to_owned(),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&common_chain, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols,
            ["__divti3"]
        );

        let weak_undefined = directory.join("weak-undefined.o");
        let strong_undefined = directory.join("strong-undefined.o");
        let strong_member = directory.join("strong-member.a");
        let weak_member = directory.join("weak-member.a");
        fs::write(
            &weak_undefined,
            fixture_object(&[("weak_target", FixtureSymbol::WeakUndefined)]),
        )
        .unwrap();
        fs::write(
            &strong_undefined,
            fixture_object(&[("weak_target", FixtureSymbol::Undefined)]),
        )
        .unwrap();
        fs::write(
            &strong_member,
            archive(&[("strong.o", symbol_object(&["weak_target"], &["__divti3"]))]),
        )
        .unwrap();
        fs::write(
            &weak_member,
            archive(&[(
                "weak.o",
                fixture_object(&[
                    ("weak_target", FixtureSymbol::Weak),
                    ("__divti3", FixtureSymbol::Undefined),
                ]),
            )]),
        )
        .unwrap();
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    weak_undefined.as_os_str().to_owned(),
                    strong_member.as_os_str().to_owned(),
                ],
                &driver,
                &config,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    strong_undefined.as_os_str().to_owned(),
                    weak_member.as_os_str().to_owned(),
                ],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"]
        );

        let notype_main = directory.join("notype-main.o");
        let notype_archive = directory.join("notype.a");
        fs::write(&notype_main, symbol_object(&[], &["asm_entry"])).unwrap();
        fs::write(
            &notype_archive,
            archive(&[(
                "asm.o",
                fixture_object(&[
                    ("asm_entry", FixtureSymbol::ZeroSizedNotype),
                    ("__divti3", FixtureSymbol::Undefined),
                ]),
            )]),
        )
        .unwrap();
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    notype_main.as_os_str().to_owned(),
                    notype_archive.as_os_str().to_owned(),
                ],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"]
        );

        let thin_member = directory.join("thinmember.o");
        let thin = directory.join("thin.a");
        fs::write(&thin_member, symbol_object(&["thin_entry"], &["__divti3"])).unwrap();
        fs::write(
            &thin,
            thin_archive(
                "thinmember.o",
                fs::metadata(&thin_member).unwrap().len() as usize,
            ),
        )
        .unwrap();
        let thin_main = directory.join("thin-main.o");
        fs::write(&thin_main, symbol_object(&[], &["thin_entry"])).unwrap();
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    thin_main.as_os_str().to_owned(),
                    thin.as_os_str().to_owned(),
                ],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"]
        );

        let suffixless_object = directory.join("suffixless-object");
        fs::write(&suffixless_object, symbol_object(&[], &["__divti3"])).unwrap();
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[suffixless_object.as_os_str().to_owned()],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"]
        );
        let suffixless_main = directory.join("suffixless-main");
        let suffixless_archive = directory.join("suffixless-archive");
        fs::write(&suffixless_main, symbol_object(&[], &["suffixless_entry"])).unwrap();
        fs::write(
            &suffixless_archive,
            archive(&[(
                "member.o",
                symbol_object(&["suffixless_entry"], &["__divti3"]),
            )]),
        )
        .unwrap();
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[
                    suffixless_main.as_os_str().to_owned(),
                    suffixless_archive.as_os_str().to_owned(),
                ],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols,
            ["__divti3"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_helper_scan_tracks_linker_state_and_uncertain_inputs() {
        let directory = env::temp_dir().join(format!(
            "ccc-runtime-helper-state-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        let driver = ToolCommandSpec::new("/definitely/not/invoked");
        let main = directory.join("main.o");
        fs::write(&main, symbol_object(&[], &["needed"])).unwrap();
        let library = directory.join("state.a");
        fs::write(
            &library,
            archive(&[
                ("selected.o", symbol_object(&["needed"], &[])),
                ("unused.o", symbol_object(&["unused"], &["__divti3"])),
            ]),
        )
        .unwrap();
        let restored_whole = [
            main.as_os_str().to_owned(),
            OsString::from("-Wl,--push-state,--whole-archive,--pop-state"),
            library.as_os_str().to_owned(),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&restored_whole, &driver, &config).unwrap(),
            None
        );

        fs::write(
            directory.join("libchoice.so"),
            symbol_object(&["needed"], &[]),
        )
        .unwrap();
        fs::write(
            directory.join("libchoice.a"),
            archive(&[("selected.o", symbol_object(&["needed"], &["__divti3"]))]),
        )
        .unwrap();
        let restored_static = [
            main.as_os_str().to_owned(),
            OsString::from("-Wl,--push-state,-dn,--pop-state"),
            OsString::from(format!("-L{}", directory.display())),
            OsString::from("-lchoice"),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&restored_static, &driver, &config).unwrap(),
            None
        );
        let global_static = [
            main.as_os_str().to_owned(),
            OsString::from(format!("-L{}", directory.display())),
            OsString::from("-lchoice"),
            OsString::from("-static"),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&global_static, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols,
            ["__divti3"],
            "driver static mode applies before libraries regardless of argv position"
        );
        let positional_static_after = [
            main.as_os_str().to_owned(),
            OsString::from(format!("-L{}", directory.display())),
            OsString::from("-lchoice"),
            OsString::from("-Wl,-static"),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&positional_static_after, &driver, &config)
                .unwrap(),
            None,
            "linker-pass-through static mode is positional"
        );
        let positional_static_before = [
            main.as_os_str().to_owned(),
            OsString::from(format!("-L{}", directory.display())),
            OsString::from("-Xlinker"),
            OsString::from("-static"),
            OsString::from("-lchoice"),
        ];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&positional_static_before, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols,
            ["__divti3"]
        );

        let misleading_dso = directory.join("not-an-input.so");
        fs::write(&misleading_dso, b"rpath operand").unwrap();
        let rpath_only = [OsString::from(format!(
            "-Wl,-rpath,{}",
            misleading_dso.display()
        ))];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&rpath_only, &driver, &config).unwrap(),
            None
        );

        let linker_script = directory.join("layout.ld");
        fs::write(&linker_script, b"SECTIONS {}\n").unwrap();
        let uncertain = [OsString::from("-T"), linker_script.as_os_str().to_owned()];
        assert_eq!(
            runtime_helper_link_plan_for_inputs(&uncertain, &driver, &config)
                .unwrap()
                .unwrap()
                .symbols
                .len(),
            16
        );
        for inputs in [
            vec![linker_script.as_os_str().to_owned()],
            vec![OsString::from("-dT"), linker_script.as_os_str().to_owned()],
            vec![OsString::from(format!(
                "--default-script={}",
                linker_script.display()
            ))],
        ] {
            assert_eq!(
                runtime_helper_link_plan_for_inputs(&inputs, &driver, &config)
                    .unwrap()
                    .unwrap()
                    .symbols
                    .len(),
                16
            );
        }
        assert_eq!(
            runtime_helper_link_plan_for_inputs(
                &[OsString::from("-ldefinitely_missing_ccc_fixture")],
                &driver,
                &config,
            )
            .unwrap()
            .unwrap()
            .symbols
            .len(),
            16
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dynamic_definitions_follow_as_needed_at_their_link_position() {
        let main_x = fixture_object(&[("x", FixtureSymbol::Undefined)]);
        let main_helper = fixture_object(&[("__divti3", FixtureSymbol::Undefined)]);
        let object = object::File::parse(main_x.as_slice()).unwrap();
        let mut state = LinkSymbolState::default();
        process_object_facts(&mut state, &object_symbol_facts(&object));
        apply_dynamic_definitions(&mut state, HashSet::from(["x".to_owned()]), false);
        let archive_bytes = archive(&[("both.o", symbol_object(&["x", "__divti3"], &[]))]);
        process_archive(Path::new("fixture.a"), &archive_bytes, &mut state, false).unwrap();
        let object = object::File::parse(main_helper.as_slice()).unwrap();
        process_object_facts(&mut state, &object_symbol_facts(&object));
        assert!(state.unresolved.contains("__divti3"));

        let mut dropped = LinkSymbolState::default();
        apply_dynamic_definitions(&mut dropped, HashSet::from(["x".to_owned()]), true);
        let object = object::File::parse(main_x.as_slice()).unwrap();
        process_object_facts(&mut dropped, &object_symbol_facts(&object));
        let fallback = archive(&[("fallback.o", symbol_object(&["x"], &["__divti3"]))]);
        process_archive(Path::new("fallback.a"), &fallback, &mut dropped, false).unwrap();
        assert!(dropped.unresolved.contains("__divti3"));

        let mut retained = LinkSymbolState::default();
        let object = object::File::parse(main_x.as_slice()).unwrap();
        process_object_facts(&mut retained, &object_symbol_facts(&object));
        apply_dynamic_definitions(&mut retained, HashSet::from(["x".to_owned()]), true);
        assert!(!retained.unresolved.contains("x"));
        assert!(retained.dynamic_defined.contains("x"));
    }

    #[test]
    fn compiler_runtime_provider_isolated_from_user_whole_archive_state() {
        let mut command = Command::new("cc");
        let providers = BTreeSet::from([PathBuf::from("/runtime/libgcc.a")]);
        append_runtime_helper_providers(&mut command, &providers, ccc_target::BinaryFormat::Elf);
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-Wl,--push-state,--no-whole-archive",
                "/runtime/libgcc.a",
                "-Wl,--pop-state",
            ]
        );
    }

    #[test]
    fn runtime_helper_provider_diagnostics_are_deterministic_for_bad_archives() {
        let plan = RuntimeHelperLinkPlan {
            provider: RuntimeHelperProvider::CompilerBuiltins,
            symbols: vec!["__divti3", "__fixdfti"],
        };
        let missing = verify_runtime_helper_symbols(
            Path::new("/fixture/compiler-builtins.a"),
            &HashSet::new(),
            &plan,
        )
        .unwrap_err();
        assert_eq!(missing.code, "CCC5008");
        assert_eq!(
            missing.message,
            "compiler builtins provider `/fixture/compiler-builtins.a` is missing required runtime helpers: __divti3, __fixdfti"
        );

        let path = env::temp_dir().join(format!(
            "ccc-malformed-builtins-{}-{}.a",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"not an archive").unwrap();
        let malformed = verify_runtime_helper_archive(&path, &plan).unwrap_err();
        fs::remove_file(&path).unwrap();
        assert_eq!(malformed.code, "CCC5008");
        assert!(
            malformed
                .message
                .starts_with("resolved compiler builtins provider `")
        );
        assert!(malformed.message.contains("is not a readable archive:"));
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
    fn ambient_build_cc_does_not_select_ccc_as_its_own_toolchain_driver() {
        let environment = relevant_environment_with(|name| match name {
            "CC" => Some(OsString::from("ccc")),
            "CCC_CC" => None,
            _ => None,
        });
        assert!(environment.iter().all(|entry| entry.name != "CC"));

        let config = EffectiveCompilationConfig::default();
        let driver = driver_from_environment(&environment, &config.target.triple).unwrap();
        assert_ne!(driver.program, PathBuf::from("ccc"));
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
