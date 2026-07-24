use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
use std::ffi::{OsStr, OsString};

use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, RelocationFlags, RelocationTarget,
    SectionKind, SymbolKind,
};

mod support;

fn compile_ccc(source: &Path, output: &Path) {
    compile_ccc_with_options(source, output, &[]);
}

fn compile_ccc_with_options(source: &Path, output: &Path, options: &[&str]) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
    command.arg("-nostdinc").arg("-c").args(options);
    let result = command.arg(source).arg("-o").arg(output).output().unwrap();
    support::assert_command_success("compile a native object with CCC", &result);
}

fn compile_x86_64_elf_with_options(source: &Path, output: &Path, options: &[&str]) {
    let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("--target=x86_64-unknown-linux-gnu")
        .arg("-nostdinc")
        .arg("-c")
        .args(options)
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap();
    support::assert_command_success("compile an x86-64 ELF object with CCC", &result);
}

fn native_section_name(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("__{}", name.trim_start_matches('.'))
    } else {
        name.to_owned()
    }
}

fn native_symbol_name(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

#[test]
fn debug_levels_emit_source_types_variables_and_relocations() {
    let directory =
        support::TestWorkspace::new("object-emission", "quality-options").retain_on_failure();
    let source = directory.join("quality-options.c");
    let optimized = directory.join("optimized.o");
    let disabled = directory.join("disabled.o");
    let with_debug = directory.join("with-debug.o");
    fs::write(
        &source,
        r#"
struct Pair { int left; long right; };
int global_value;

int inspect(int *parameter) {
    volatile struct Pair local = { 3, 5 };
    int **addressable_parameter = &parameter;
    global_value = local.left;
    return **addressable_parameter + (int)local.right;
}
"#,
    )
    .unwrap();

    compile_ccc_with_options(&source, &optimized, &["-Oz"]);
    compile_ccc_with_options(&source, &disabled, &["-g", "-g0", "-Oz"]);
    compile_ccc_with_options(&source, &with_debug, &["-g3", "-Oz"]);

    assert_eq!(fs::read(&optimized).unwrap(), fs::read(&disabled).unwrap());
    let bytes = fs::read(&with_debug).unwrap();
    assert_ne!(fs::read(&optimized).unwrap(), bytes);
    for (index, level) in ["-g", "-g1", "-g2"].into_iter().enumerate() {
        let output = directory.join(format!("debug-level-{index}.o"));
        compile_ccc_with_options(&source, &output, &[level, "-Oz"]);
        assert_eq!(fs::read(output).unwrap(), bytes, "{level} debug profile");
    }
    let file = object::File::parse(bytes.as_slice()).unwrap();
    for section in [".debug_abbrev", ".debug_info", ".debug_line", ".eh_frame"] {
        assert!(
            file.section_by_name(&native_section_name(section))
                .is_some(),
            "missing {section}"
        );
    }
    if cfg!(target_os = "linux") {
        assert!(
            file.section_by_name(&native_section_name(".debug_ranges"))
                .is_some(),
            "missing .debug_ranges"
        );
    }
    let debug_relocations = [".debug_info", ".debug_line", ".debug_ranges"]
        .into_iter()
        .filter_map(|name| {
            file.section_by_name(&native_section_name(name))
                .map(|section| section.relocations())
        })
        .flatten()
        .count();
    assert!(
        debug_relocations >= 5,
        "expected relocation-bearing DWARF, found {debug_relocations} relocations"
    );

    let sections = gimli::DwarfSections::load(|id| {
        let name = native_section_name(id.name());
        Ok::<_, gimli::Error>(
            file.section_by_name(&name)
                .and_then(|section| section.data().ok())
                .unwrap_or_default()
                .to_vec(),
        )
    })
    .unwrap();
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, gimli::LittleEndian));
    let mut units = dwarf.units();
    let header = units.next().unwrap().expect("debug compilation unit");
    assert!(
        units.next().unwrap().is_none(),
        "one source should produce one unit"
    );
    let unit = dwarf.unit(header).unwrap();
    let mut entries = unit.entries();
    let mut tags = std::collections::HashMap::new();
    let mut named = std::collections::BTreeSet::new();
    let mut located_variables = 0;
    while let Some(entry) = entries.next_dfs().unwrap() {
        *tags.entry(entry.tag()).or_insert(0_usize) += 1;
        if entry.tag() == gimli::DW_TAG_variable && entry.has_attr(gimli::DW_AT_location) {
            located_variables += 1;
        }
        if let Some(attribute) = entry.attr(gimli::DW_AT_name)
            && let Ok(name) = dwarf.attr_string(&unit, attribute.value())
        {
            named.insert(name.to_string_lossy().into_owned());
        }
    }
    for tag in [
        gimli::DW_TAG_compile_unit,
        gimli::DW_TAG_subprogram,
        gimli::DW_TAG_structure_type,
        gimli::DW_TAG_member,
        gimli::DW_TAG_formal_parameter,
        gimli::DW_TAG_lexical_block,
        gimli::DW_TAG_variable,
        gimli::DW_TAG_base_type,
    ] {
        assert!(
            tags.get(&tag).is_some_and(|count| *count > 0),
            "missing {tag:?}"
        );
    }
    for name in [
        "Pair",
        "left",
        "right",
        "inspect",
        "parameter",
        "local",
        "global_value",
    ] {
        assert!(
            named.contains(name),
            "missing debug name `{name}`: {named:?}"
        );
    }
    assert!(
        located_variables >= 2,
        "addressable variables need locations"
    );

    let mut rows = unit.line_program.clone().expect("line program").rows();
    let mut source_rows = 0;
    while let Some((_, row)) = rows.next_row().unwrap() {
        if !row.end_sequence() && row.line().map(|line| line.get()).unwrap_or(0) > 0 {
            source_rows += 1;
        }
    }
    assert!(source_rows >= 3, "expected source-level line rows");
}

#[test]
fn optimization_profiles_emit_valid_objects_and_o0_preserves_default() {
    let directory =
        support::TestWorkspace::new("object-emission", "optimization-options").retain_on_failure();
    let source = directory.join("optimization-options.c");
    let baseline = directory.join("baseline.o");
    let unoptimized = directory.join("unoptimized.o");
    fs::write(
        &source,
        "int answer(int condition) { if (condition) return 42; return 7; }\n",
    )
    .unwrap();

    compile_ccc(&source, &baseline);
    compile_ccc_with_options(&source, &unoptimized, &["-O0"]);

    assert_eq!(
        fs::read(&baseline).unwrap(),
        fs::read(&unoptimized).unwrap()
    );
    let mut optimized_objects = std::collections::BTreeMap::new();
    for optimization in ["-O", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        let output = directory.join(format!("{}.o", &optimization[1..]));
        compile_ccc_with_options(&source, &output, &[optimization]);
        let bytes = fs::read(output).unwrap();
        object::File::parse(bytes.as_slice()).unwrap();
        optimized_objects.insert(optimization, bytes);
    }
    for (left, right) in [("-O", "-O1"), ("-O2", "-O3"), ("-Os", "-Oz")] {
        assert_eq!(
            optimized_objects[left], optimized_objects[right],
            "{left} and {right} promise the same pass set"
        );
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn objects_built_at_different_optimization_levels_link_and_execute_together() {
    let directory = support::TestWorkspace::new("object-emission", "mixed-optimization-link")
        .retain_on_failure();
    let library_source = directory.join("optimized-library.c");
    let main_source = directory.join("unoptimized-main.c");
    let library_object = directory.join("optimized-library.o");
    let main_object = directory.join("unoptimized-main.o");
    let executable = directory.join("mixed-optimization");
    fs::write(
        &library_source,
        "int optimized_answer(int value) { return (value + 1) + (value + 1); }\n",
    )
    .unwrap();
    fs::write(
        &main_source,
        "extern int optimized_answer(int);\n\
         int main(void) { return optimized_answer(20) == 42 ? 0 : 1; }\n",
    )
    .unwrap();

    for (source, object, optimization) in [
        (&library_source, &library_object, "-Oz"),
        (&main_source, &main_object, "-O0"),
    ] {
        let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .arg("-nostdinc")
            .arg("-c")
            .arg(optimization)
            .arg(source)
            .arg("-o")
            .arg(object)
            .output()
            .unwrap();
        directory.assert_command_success("compile one mixed-profile object", &compilation);
    }
    let target_cc = std::env::var_os("CCC_CC").unwrap_or_else(|| "cc".into());
    let link = Command::new(target_cc)
        .arg("-o")
        .arg(&executable)
        .arg(&main_object)
        .arg(&library_object)
        .output()
        .unwrap();
    directory.assert_command_success("link mixed-profile objects", &link);
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(0),
        "mixed-profile executable failed:\nstdout:\n{}\nstderr:\n{}\nworkspace: {}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr),
        directory.path().display()
    );
}

#[test]
fn optimization_runs_before_ir_dump_and_abi_planning() {
    let directory =
        support::TestWorkspace::new("object-emission", "optimization-ir").retain_on_failure();
    let source = directory.join("optimization-ir.c");
    fs::write(
        &source,
        "int inspect(volatile int *observed, int *plain, int count) {\n\
             int unused = *plain;\n\
             int values[count];\n\
             *observed;\n\
             if (0) return unused;\n\
             return 0;\n\
         }\n",
    )
    .unwrap();

    let dump = |optimization: &str, representation: &str| {
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .args(["-nostdinc", optimization, representation])
            .arg(&source)
            .output()
            .unwrap();
        directory.assert_command_success("dump optimized CCC IR", &result);
        String::from_utf8(result.stdout).unwrap()
    };

    let unoptimized = dump("-O0", "--dump-ir");
    let optimized = dump("-O2", "--dump-ir");
    assert!(unoptimized.contains("conditional"), "{unoptimized}");
    assert!(!optimized.contains("conditional"), "{optimized}");
    assert!(optimized.contains("runtime.allocate"), "{optimized}");
    assert!(optimized.contains("volatile=true"), "{optimized}");
    assert!(!dump("-O2", "--dump-abi").is_empty());
}

#[test]
fn debug_information_covers_data_only_translation_units() {
    let directory =
        support::TestWorkspace::new("object-emission", "debug-data-only").retain_on_failure();
    let source = directory.join("debug-data-only.c");
    let output = directory.join("debug-data-only.o");
    fs::write(
        &source,
        "struct Item { int value; }; struct Item global_item;\n",
    )
    .unwrap();

    compile_ccc_with_options(&source, &output, &["-g"]);
    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let debug_info = native_section_name(".debug_info");
    assert!(file.section_by_name(&debug_info).is_some());
    assert!(
        file.section_by_name(&debug_info)
            .unwrap()
            .relocations()
            .count()
            >= if cfg!(target_os = "macos") { 1 } else { 2 },
        "the compilation unit and global address must be relocatable"
    );
}

#[test]
fn x86_64_elf_objects_use_position_independent_text_relocations() {
    let directory =
        support::TestWorkspace::new("object-emission", "position-independent-relocations")
            .retain_on_failure();
    let source = directory.join("position-independent-relocations.c");
    let output = directory.join("position-independent-relocations.o");
    fs::write(
        &source,
        r#"
extern int imported_data;
extern int imported_function(void);
int local_data = 7;

int *local_address(void) {
    return &local_data;
}

int read_imported(void) {
    return imported_data;
}

int call_imported(void) {
    return imported_function();
}
"#,
    )
    .unwrap();
    compile_x86_64_elf_with_options(&source, &output, &[]);

    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let text = file.section_by_name(".text").unwrap();
    let relocations = text
        .relocations()
        .filter_map(|(_, relocation)| {
            let RelocationTarget::Symbol(index) = relocation.target() else {
                return None;
            };
            let name = file.symbol_by_index(index).ok()?.name().ok()?;
            Some((name.to_owned(), relocation.flags()))
        })
        .collect::<Vec<_>>();

    for name in ["local_data", "imported_data"] {
        assert!(
            relocations.iter().any(|(target, flags)| {
                target == name
                    && *flags
                        == RelocationFlags::Elf {
                            r_type: object::elf::R_X86_64_GOTPCREL,
                        }
            }),
            "missing position-independent data relocation to `{name}`: {relocations:?}"
        );
    }
    assert!(
        relocations.iter().any(|(target, flags)| {
            target == "imported_function"
                && *flags
                    == RelocationFlags::Elf {
                        r_type: object::elf::R_X86_64_PLT32,
                    }
        }),
        "missing PLT-relative call relocation: {relocations:?}"
    );
    assert!(
        relocations.iter().all(|(_, flags)| !matches!(
            flags,
            RelocationFlags::Elf {
                r_type: object::elf::R_X86_64_32 | object::elf::R_X86_64_32S
            }
        )),
        "position-independent text contains an absolute relocation: {relocations:?}"
    );
}

// This pins the exact AMD64 relocation vocabulary. The target-oracle runner
// checks the corresponding AArch64 and RISC-V ELF relocation families.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn thread_local_objects_use_the_selected_elf_relocation_models() {
    let directory = support::TestWorkspace::new("object-emission", "thread-local-relocations")
        .retain_on_failure();
    let source = directory.join("thread-local-relocations.c");
    let output = directory.join("thread-local-relocations.o");
    fs::write(
        &source,
        r#"
_Thread_local int global_dynamic
    __attribute__((tls_model("global-dynamic"))) = 1;
_Thread_local int local_dynamic
    __attribute__((tls_model("local-dynamic"))) = 2;
_Thread_local int initial_exec
    __attribute__((tls_model("initial-exec"))) = 3;
_Thread_local int local_exec
    __attribute__((tls_model("local-exec"))) = 4;
_Thread_local int zero_tls;

int *block_tls_address(void) {
    static _Thread_local int block_tls = 8;
    return &block_tls;
}

int read_tls_models(void) {
    return global_dynamic + local_dynamic + initial_exec + local_exec + zero_tls;
}
"#,
    )
    .unwrap();
    compile_x86_64_elf_with_options(&source, &output, &["-g"]);

    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    for name in [
        "global_dynamic",
        "local_dynamic",
        "initial_exec",
        "local_exec",
    ] {
        let symbol = file
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing TLS symbol `{name}`"));
        assert_eq!(symbol.kind(), SymbolKind::Tls, "{name}");
        let section = file
            .section_by_index(symbol.section_index().expect("defined TLS symbol"))
            .unwrap();
        assert_eq!(section.name().unwrap(), ".tdata", "{name}");
    }
    let zero = file
        .symbol_by_name("zero_tls")
        .expect("missing zero TLS symbol");
    assert_eq!(zero.kind(), SymbolKind::Tls);
    assert_eq!(
        file.section_by_index(zero.section_index().expect("defined zero TLS symbol"))
            .unwrap()
            .name()
            .unwrap(),
        ".tbss"
    );
    let block_name = "__ccc_block_static.block_tls_address.0.0.block_tls";
    let block = file
        .symbol_by_name(block_name)
        .expect("missing block-local TLS symbol");
    assert_eq!(block.kind(), SymbolKind::Tls);
    assert!(block.is_local(), "block-local TLS must be localized");
    assert_eq!(
        file.section_by_index(block.section_index().expect("defined block TLS symbol"))
            .unwrap()
            .name()
            .unwrap(),
        ".tdata"
    );

    let relocation_types = file
        .sections()
        .flat_map(|section| section.relocations())
        .filter_map(|(_, relocation)| match relocation.flags() {
            RelocationFlags::Elf { r_type } => Some(r_type),
            _ => None,
        })
        .collect::<Vec<_>>();
    for expected in [
        object::elf::R_X86_64_TLSGD,
        object::elf::R_X86_64_TLSLD,
        object::elf::R_X86_64_DTPOFF32,
        object::elf::R_X86_64_DTPOFF64,
        object::elf::R_X86_64_GOTTPOFF,
        object::elf::R_X86_64_TPOFF32,
    ] {
        assert!(
            relocation_types.contains(&expected),
            "missing TLS relocation {expected}: {relocation_types:?}"
        );
    }
    let accessors = file
        .symbols()
        .filter(|symbol| {
            symbol
                .name()
                .is_ok_and(|name| name.starts_with("__ccc_tls_accessor_"))
        })
        .collect::<Vec<_>>();
    assert_eq!(accessors.len(), 6);
    assert!(
        accessors
            .iter()
            .all(|symbol| symbol.is_local() && symbol.is_definition()),
        "packaged TLS accessors must be localized definitions"
    );
}

#[test]
fn x86_64_elf_object_has_expected_sections_bindings_and_relocations() {
    let directory = support::TestWorkspace::new("object-emission", "structure").retain_on_failure();
    let source = directory.join("structure.c");
    let output = directory.join("structure.o");
    fs::write(
        &source,
        r#"
extern int imported(int);
int exported_data = 7;
static int internal_data = 9;
static int zero_data;
int *data_pointer = &exported_data;
static const char *message = "hello";

int call_imported(void) {
    return imported(message[0] + internal_data);
}
"#,
    )
    .unwrap();
    compile_x86_64_elf_with_options(&source, &output, &[]);

    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();

    for (name, expected_kind) in [
        (".text", SectionKind::Text),
        (".data", SectionKind::Data),
        (".bss", SectionKind::UninitializedData),
        (".rodata", SectionKind::ReadOnlyData),
    ] {
        let section = file
            .section_by_name(name)
            .unwrap_or_else(|| panic!("missing {name} section"));
        assert_eq!(section.kind(), expected_kind, "wrong kind for {name}");
    }

    let exported = file.symbol_by_name("exported_data").unwrap();
    assert!(exported.is_definition());
    assert!(exported.is_global());
    assert_eq!(exported.kind(), SymbolKind::Data);

    let internal = file.symbol_by_name("internal_data").unwrap();
    assert!(internal.is_definition());
    assert!(internal.is_local());
    assert_eq!(internal.kind(), SymbolKind::Data);

    let zero = file.symbol_by_name("zero_data").unwrap();
    assert_eq!(
        file.section_by_index(zero.section_index().unwrap())
            .unwrap()
            .kind(),
        SectionKind::UninitializedData
    );

    let imported = file.symbol_by_name("imported").unwrap();
    assert!(imported.is_undefined());
    assert!(imported.is_global());

    let mut relocation_targets = Vec::new();
    let mut imported_relocation = None;
    for section in file.sections() {
        for (_, relocation) in section.relocations() {
            let RelocationTarget::Symbol(index) = relocation.target() else {
                continue;
            };
            let name = file
                .symbol_by_index(index)
                .unwrap()
                .name()
                .unwrap()
                .to_owned();
            relocation_targets.push(name.clone());
            if name == "imported" {
                imported_relocation = Some(relocation.flags());
            }
        }
    }
    assert!(
        relocation_targets
            .iter()
            .any(|name| name == "exported_data"),
        "data pointer has no relocation to exported_data: {relocation_targets:?}"
    );
    let string_symbol = file.symbol_by_name("__ccc_string_0").unwrap();
    assert!(string_symbol.is_definition());
    assert!(string_symbol.is_local());
    assert_eq!(
        file.section_by_index(string_symbol.section_index().unwrap())
            .unwrap()
            .kind(),
        SectionKind::ReadOnlyData
    );
    assert!(
        relocation_targets
            .iter()
            .any(|name| name.starts_with("__ccc_string_")),
        "string pointer has no relocation to pooled data: {relocation_targets:?}"
    );
    assert_eq!(
        imported_relocation,
        Some(RelocationFlags::Elf {
            r_type: object::elf::R_X86_64_PLT32,
        }),
        "direct external calls must use the linker's procedure linkage table"
    );
}

#[test]
fn incomplete_extern_arrays_emit_only_undefined_data_symbols() {
    let directory = support::TestWorkspace::new("object-emission", "incomplete-extern-arrays")
        .retain_on_failure();
    let source = directory.join("incomplete-extern-arrays.c");
    let output = directory.join("incomplete-extern-arrays.o");
    fs::write(
        &source,
        "extern const char bytes[];\n\
         extern int values[];\n\
         int read_imports(void) { return bytes[0] + values[0]; }\n",
    )
    .unwrap();
    compile_ccc(&source, &output);

    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    for name in ["bytes", "values"] {
        let symbol_name = native_symbol_name(name);
        let symbol = file
            .symbol_by_name(&symbol_name)
            .unwrap_or_else(|| panic!("missing `{name}`"));
        assert!(symbol.is_undefined(), "{name}");
        assert!(symbol.is_global(), "{name}");
        if cfg!(target_os = "macos") {
            assert_eq!(symbol.kind(), SymbolKind::Unknown, "{name}");
        } else {
            assert_eq!(symbol.kind(), SymbolKind::Data, "{name}");
        }
        assert_eq!(symbol.size(), 0, "{name}");
    }
}

#[test]
fn x86_64_elf_declaration_assembly_labels_control_defined_and_referenced_symbols() {
    let directory = support::TestWorkspace::new("object-emission", "declaration-assembly-labels")
        .retain_on_failure();
    let source = directory.join("declaration-assembly-labels.c");
    let output = directory.join("declaration-assembly-labels.o");
    fs::write(
        &source,
        r#"
extern int source_function(int) asm("linked_function");
extern int source_object asm("linked_object");

int exported_function(int) asm("renamed_function");
int exported_function(int value) {
    return source_function(value) + source_object;
}

int exported_object asm("renamed_object") = 7;
"#,
    )
    .unwrap();
    compile_x86_64_elf_with_options(&source, &output, &[]);

    let bytes = fs::read(&output).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    for (name, kind) in [
        ("renamed_function", SymbolKind::Text),
        ("renamed_object", SymbolKind::Data),
    ] {
        let symbol = file
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing defined assembly-label symbol `{name}`"));
        assert!(symbol.is_definition(), "`{name}` must be defined");
        assert!(symbol.is_global(), "`{name}` must have external linkage");
        assert_eq!(symbol.kind(), kind, "wrong symbol kind for `{name}`");
    }
    for name in ["linked_function", "linked_object"] {
        let symbol = file
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing referenced assembly-label symbol `{name}`"));
        assert!(symbol.is_undefined(), "`{name}` must remain undefined");
        assert!(symbol.is_global(), "`{name}` must have external linkage");
    }
    for source_name in [
        "source_function",
        "source_object",
        "exported_function",
        "exported_object",
    ] {
        assert!(
            file.symbol_by_name(source_name).is_none(),
            "C lookup name `{source_name}` leaked into the object symbol table"
        );
    }

    let relocation_targets = file
        .sections()
        .flat_map(|section| section.relocations())
        .filter_map(|(_, relocation)| match relocation.target() {
            RelocationTarget::Symbol(index) => file
                .symbol_by_index(index)
                .ok()
                .and_then(|symbol| symbol.name().ok())
                .map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>();
    for name in ["linked_function", "linked_object"] {
        assert!(
            relocation_targets.iter().any(|target| target == name),
            "missing relocation to `{name}`: {relocation_targets:?}"
        );
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[derive(Clone, Debug)]
struct ReferenceCompiler {
    program: OsString,
    arguments: Vec<OsString>,
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
impl ReferenceCompiler {
    fn required() -> Self {
        let value = std::env::var_os("CCC_CC")
            .or_else(|| std::env::var_os("CC"))
            .unwrap_or_else(|| OsString::from("cc"));
        let mut words = value
            .to_string_lossy()
            .split_whitespace()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert!(!words.is_empty(), "CC must name a compiler driver");
        Self {
            program: words.remove(0),
            arguments: words,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.arguments);
        command
    }

    fn identity(&self) -> String {
        let output = self
            .command()
            .arg("--version")
            .output()
            .unwrap_or_else(|error| panic!("required reference compiler is unavailable: {error}"));
        support::assert_command_success("query the reference compiler version", &output);
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("<no version text>")
            .to_owned()
    }

    fn compile(&self, source: &Path, output: &Path) {
        let result = self
            .command()
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(output)
            .output()
            .unwrap();
        support::assert_command_success("compile an object with the reference compiler", &result);
    }

    fn link<I, P>(&self, objects: I, output: &Path)
    where
        I: IntoIterator<Item = P>,
        P: AsRef<OsStr>,
    {
        let result = self
            .command()
            .args(objects)
            .arg("-o")
            .arg(output)
            .output()
            .unwrap();
        support::assert_command_success("link objects with the reference compiler", &result);
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
fn write_source(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
fn run_successfully(executable: &Path) {
    let result = Command::new(executable).output().unwrap();
    support::assert_command_success("run a cross-linked executable", &result);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn objects_cross_link_in_both_directions_and_keep_static_names_local() {
    let directory =
        support::TestWorkspace::new("object-emission", "cross-link").retain_on_failure();
    let reference = ReferenceCompiler::required();
    eprintln!("cross-link reference compiler: {}", reference.identity());

    let ccc_caller = write_source(
        directory.path(),
        "ccc-caller.c",
        "extern int reference_add(int); int main(void) { return reference_add(35) == 42 ? 0 : 1; }\n",
    );
    let reference_callee = write_source(
        directory.path(),
        "reference-callee.c",
        "int reference_add(int value) { return value + 7; }\n",
    );
    let ccc_caller_object = directory.join("ccc-caller.o");
    let reference_callee_object = directory.join("reference-callee.o");
    compile_ccc(&ccc_caller, &ccc_caller_object);
    reference.compile(&reference_callee, &reference_callee_object);
    let caller_program = directory.join("ccc-caller");
    reference.link(
        [&ccc_caller_object, &reference_callee_object],
        &caller_program,
    );
    run_successfully(&caller_program);

    let ccc_callee = write_source(
        directory.path(),
        "ccc-callee.c",
        "int ccc_multiply(int left, int right) { return left * right; }\n",
    );
    let reference_caller = write_source(
        directory.path(),
        "reference-caller.c",
        "extern int ccc_multiply(int, int); int main(void) { return ccc_multiply(6, 7) == 42 ? 0 : 1; }\n",
    );
    let ccc_callee_object = directory.join("ccc-callee.o");
    let reference_caller_object = directory.join("reference-caller.o");
    compile_ccc(&ccc_callee, &ccc_callee_object);
    reference.compile(&reference_caller, &reference_caller_object);
    let callee_program = directory.join("ccc-callee");
    reference.link(
        [&reference_caller_object, &ccc_callee_object],
        &callee_program,
    );
    run_successfully(&callee_program);

    let left = write_source(
        directory.path(),
        "left.c",
        "static int hidden = 19; int left(void) { return hidden; }\n",
    );
    let right = write_source(
        directory.path(),
        "right.c",
        "static int hidden = 23; int right(void) { return hidden; }\n",
    );
    let local_main = write_source(
        directory.path(),
        "local-main.c",
        "extern int left(void); extern int right(void); int main(void) { return left() + right() == 42 ? 0 : 1; }\n",
    );
    let left_object = directory.join("left.o");
    let right_object = directory.join("right.o");
    let local_main_object = directory.join("local-main.o");
    compile_ccc(&left, &left_object);
    compile_ccc(&right, &right_object);
    reference.compile(&local_main, &local_main_object);
    let local_program = directory.join("local-symbols");
    reference.link(
        [&local_main_object, &left_object, &right_object],
        &local_program,
    );
    run_successfully(&local_program);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn thread_local_objects_cross_link_with_the_platform_compiler() {
    let directory = support::TestWorkspace::new("object-emission", "thread-local-cross-link")
        .retain_on_failure();
    let reference = ReferenceCompiler::required();
    eprintln!("TLS reference compiler: {}", reference.identity());

    let ccc_definition = write_source(
        directory.path(),
        "ccc-tls-definition.c",
        "_Thread_local int ccc_tls_value = 17;\n\
         int ccc_tls_read(void) { return ccc_tls_value; }\n",
    );
    let reference_consumer = write_source(
        directory.path(),
        "reference-tls-consumer.c",
        "extern _Thread_local int ccc_tls_value;\n\
         extern int ccc_tls_read(void);\n\
         int main(void) { ccc_tls_value = 42; return ccc_tls_read() == 42 ? 0 : 1; }\n",
    );
    let ccc_definition_object = directory.join("ccc-tls-definition.o");
    let reference_consumer_object = directory.join("reference-tls-consumer.o");
    compile_ccc(&ccc_definition, &ccc_definition_object);
    reference.compile(&reference_consumer, &reference_consumer_object);
    let ccc_definition_program = directory.join("ccc-tls-definition");
    reference.link(
        [&reference_consumer_object, &ccc_definition_object],
        &ccc_definition_program,
    );
    run_successfully(&ccc_definition_program);

    let ccc_reader = write_source(
        directory.path(),
        "ccc-tls-reader.c",
        "_Thread_local int reference_tls_value __attribute__((weak));\n\
         int reference_tls_read(void) { return reference_tls_value; }\n",
    );
    let reference_definition = write_source(
        directory.path(),
        "reference-tls-definition.c",
        "_Thread_local int reference_tls_value = 39;\n\
         extern int reference_tls_read(void);\n\
         int main(void) { return reference_tls_read() == 39 ? 0 : 1; }\n",
    );
    let ccc_reader_object = directory.join("ccc-tls-reader.o");
    let reference_definition_object = directory.join("reference-tls-definition.o");
    compile_ccc(&ccc_reader, &ccc_reader_object);
    reference.compile(&reference_definition, &reference_definition_object);
    let reference_definition_program = directory.join("reference-tls-definition");
    reference.link(
        [&reference_definition_object, &ccc_reader_object],
        &reference_definition_program,
    );
    run_successfully(&reference_definition_program);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn weak_definitions_link_as_fallbacks_for_strong_symbols() {
    let directory = support::TestWorkspace::new("object-emission", "weak-definition-interop")
        .retain_on_failure();
    let reference = ReferenceCompiler::required();
    eprintln!("weak-symbol reference compiler: {}", reference.identity());

    let weak_source = write_source(
        directory.path(),
        "weak.c",
        "int selected(void) __attribute__((weak));\n\
         int selected(void) { return 1; }\n\
         int selected_value __attribute__((weak)) = 2;\n\
         int observe_selection(void) { return selected() + selected_value; }\n",
    );
    let fallback_main = write_source(
        directory.path(),
        "fallback-main.c",
        "extern int observe_selection(void);\n\
         int main(void) { return observe_selection() == 3 ? 0 : 1; }\n",
    );
    let strong_main = write_source(
        directory.path(),
        "strong-main.c",
        "int selected(void) { return 40; } int selected_value = 2;\n\
         extern int observe_selection(void);\n\
         int main(void) { return observe_selection() == 42 ? 0 : 1; }\n",
    );
    let weak_object = directory.join("weak.o");
    let fallback_main_object = directory.join("fallback-main.o");
    let strong_main_object = directory.join("strong-main.o");
    compile_ccc(&weak_source, &weak_object);
    reference.compile(&fallback_main, &fallback_main_object);
    reference.compile(&strong_main, &strong_main_object);

    let fallback_program = directory.join("weak-fallback");
    reference.link([&fallback_main_object, &weak_object], &fallback_program);
    run_successfully(&fallback_program);

    let override_program = directory.join("strong-override");
    reference.link([&strong_main_object, &weak_object], &override_program);
    run_successfully(&override_program);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn declaration_assembly_labels_interoperate_in_both_directions() {
    let directory =
        support::TestWorkspace::new("object-emission", "declaration-assembly-label-interop")
            .retain_on_failure();
    let reference = ReferenceCompiler::required();
    eprintln!(
        "assembly-label reference compiler: {}",
        reference.identity()
    );

    let reference_add_symbol = native_symbol_name("reference_add_impl");
    let reference_value_symbol = native_symbol_name("reference_value_impl");
    let ccc_caller_source = format!(
        r#"
extern int public_add(int) asm("{reference_add_symbol}");
extern int public_value asm("{reference_value_symbol}");
int main(void) {{
    return public_add(35) + public_value == 49 ? 0 : 1;
}}
"#,
    );
    let ccc_caller = write_source(directory.path(), "ccc-caller.c", &ccc_caller_source);
    let reference_callee = write_source(
        directory.path(),
        "reference-callee.c",
        "int reference_add_impl(int value) { return value + 7; } int reference_value_impl = 7;\n",
    );
    let ccc_caller_object = directory.join("ccc-caller.o");
    let reference_callee_object = directory.join("reference-callee.o");
    compile_ccc(&ccc_caller, &ccc_caller_object);
    reference.compile(&reference_callee, &reference_callee_object);
    let caller_program = directory.join("ccc-caller");
    reference.link(
        [&ccc_caller_object, &reference_callee_object],
        &caller_program,
    );
    run_successfully(&caller_program);

    let ccc_multiply_symbol = native_symbol_name("ccc_multiply_impl");
    let ccc_value_symbol = native_symbol_name("ccc_value_impl");
    let ccc_callee_source = format!(
        r#"
int internal_multiply(int, int) asm("{ccc_multiply_symbol}");
int internal_multiply(int left, int right) {{ return left * right; }}
int internal_value asm("{ccc_value_symbol}") = 6;
"#,
    );
    let ccc_callee = write_source(directory.path(), "ccc-callee.c", &ccc_callee_source);
    let reference_caller = write_source(
        directory.path(),
        "reference-caller.c",
        "extern int ccc_multiply_impl(int, int); extern int ccc_value_impl; int main(void) { return ccc_multiply_impl(6, 6) + ccc_value_impl == 42 ? 0 : 1; }\n",
    );
    let ccc_callee_object = directory.join("ccc-callee.o");
    let reference_caller_object = directory.join("reference-caller.o");
    compile_ccc(&ccc_callee, &ccc_callee_object);
    reference.compile(&reference_caller, &reference_caller_object);
    let callee_program = directory.join("ccc-callee");
    reference.link(
        [&reference_caller_object, &ccc_callee_object],
        &callee_program,
    );
    run_successfully(&callee_program);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn glibc_redirect_assembly_labels_form_elf_symbols_that_link() {
    let directory =
        support::TestWorkspace::new("object-emission", "glibc-redirect-labels").retain_on_failure();
    let reference = ReferenceCompiler::required();
    let identity = support::installed_glibc_identity();
    eprintln!(
        "glibc redirect target environment: {identity}; link driver={}",
        reference.identity(),
    );
    let source = write_source(
        directory.path(),
        "glibc-redirect-labels.c",
        r#"
#define _GNU_SOURCE 1
#define _FILE_OFFSET_BITS 64
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int value = 0;
    int descriptor = open("/dev/null", O_RDONLY);
    int converted = sscanf("42", "%d", &value);
    if (descriptor < 0) {
        return 1;
    }
    if (close(descriptor) != 0) {
        return 2;
    }
    return converted == 1 && value == 42 ? 0 : 3;
}
"#,
    );
    let object = directory.join("glibc-redirect-labels.o");
    let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    directory.assert_command_success("compile installed glibc redirects with CCC", &result);

    let bytes = fs::read(&object).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let name = "open64";
    let symbol = file
        .symbol_by_name(name)
        .unwrap_or_else(|| panic!("missing glibc redirect symbol `{name}`"));
    assert!(symbol.is_undefined(), "`{name}` must remain undefined");
    assert!(symbol.is_global(), "`{name}` must have external linkage");
    let scanf_redirects = ["__isoc99_sscanf", "__isoc23_sscanf"]
        .into_iter()
        .filter_map(|name| file.symbol_by_name(name).map(|symbol| (name, symbol)))
        .collect::<Vec<_>>();
    assert_eq!(
        scanf_redirects.len(),
        1,
        "expected exactly one glibc sscanf redirect, found {scanf_redirects:?}"
    );
    let (name, symbol) = &scanf_redirects[0];
    assert!(symbol.is_undefined(), "`{name}` must remain undefined");
    assert!(symbol.is_global(), "`{name}` must have external linkage");
    assert!(
        file.symbols()
            .filter_map(|symbol| symbol.name().ok())
            .all(|name| !name.contains("__USER_LABEL_PREFIX__")),
        "the predefined-macro spelling leaked into an ELF symbol"
    );

    let executable = directory.join("glibc-redirect-labels");
    reference.link([&object], &executable);
    run_successfully(&executable);
}
