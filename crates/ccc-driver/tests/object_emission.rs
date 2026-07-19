use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
use std::ffi::{OsStr, OsString};

use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, RelocationFlags, RelocationTarget,
    SectionKind, SymbolKind,
};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ccc-object-test-{}-{}-{name}",
        std::process::id(),
        TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn compile_ccc(source: &Path, output: &Path) {
    compile_ccc_with_options(source, output, &[]);
}

fn compile_ccc_with_options(source: &Path, output: &Path, options: &[&str]) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
    command
        .arg("--target=x86_64-unknown-linux-gnu")
        .arg("-nostdinc")
        .arg("-c")
        .args(options);
    let result = command.arg(source).arg("-o").arg(output).output().unwrap();
    assert!(
        result.status.success(),
        "CCC failed:\n{}",
        render_output(&result)
    );
}

#[test]
fn debug_levels_emit_source_types_variables_and_relocations() {
    let directory = test_directory("quality-options");
    let source = directory.join("quality-options.c");
    let baseline = directory.join("baseline.o");
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

    compile_ccc(&source, &baseline);
    compile_ccc_with_options(&source, &disabled, &["-g", "-g0", "-Oz"]);
    compile_ccc_with_options(&source, &with_debug, &["-g3", "-Oz"]);

    assert_eq!(fs::read(&baseline).unwrap(), fs::read(&disabled).unwrap());
    let bytes = fs::read(&with_debug).unwrap();
    assert_ne!(fs::read(&baseline).unwrap(), bytes);
    for (index, level) in ["-g", "-g1", "-g2"].into_iter().enumerate() {
        let output = directory.join(format!("debug-level-{index}.o"));
        compile_ccc_with_options(&source, &output, &[level, "-Oz"]);
        assert_eq!(fs::read(output).unwrap(), bytes, "{level} debug profile");
    }
    let file = object::File::parse(bytes.as_slice()).unwrap();
    for section in [
        ".debug_abbrev",
        ".debug_info",
        ".debug_line",
        ".debug_ranges",
        ".eh_frame",
    ] {
        assert!(file.section_by_name(section).is_some(), "missing {section}");
    }
    let debug_relocations = [".debug_info", ".debug_line", ".debug_ranges"]
        .into_iter()
        .flat_map(|name| file.section_by_name(name).unwrap().relocations())
        .count();
    assert!(
        debug_relocations >= 5,
        "expected relocation-bearing DWARF, found {debug_relocations} relocations"
    );

    let sections = gimli::DwarfSections::load(|id| {
        Ok::<_, gimli::Error>(
            file.section_by_name(id.name())
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

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn optimization_compatibility_options_preserve_baseline_object() {
    let directory = test_directory("optimization-options");
    let source = directory.join("optimization-options.c");
    let baseline = directory.join("baseline.o");
    let with_options = directory.join("with-options.o");
    fs::write(&source, "int answer(void) { return 42; }\n").unwrap();

    compile_ccc(&source, &baseline);
    compile_ccc_with_options(&source, &with_options, &["-Oz"]);

    assert_eq!(fs::read(baseline).unwrap(), fs::read(with_options).unwrap());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn debug_information_covers_data_only_translation_units() {
    let directory = test_directory("debug-data-only");
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
    assert!(file.section_by_name(".debug_info").is_some());
    assert!(
        file.section_by_name(".debug_info")
            .unwrap()
            .relocations()
            .count()
            >= 2,
        "the compilation unit and global address must be relocatable"
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn default_objects_use_position_independent_text_relocations() {
    let directory = test_directory("position-independent-relocations");
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
    compile_ccc(&source, &output);

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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn thread_local_objects_use_the_selected_elf_relocation_models() {
    let directory = test_directory("thread-local-relocations");
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
    compile_ccc_with_options(&source, &output, &["-g"]);

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
    fs::remove_dir_all(directory).unwrap();
}

fn render_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn emitted_object_has_expected_sections_bindings_and_relocations() {
    let directory = test_directory("structure");
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
    compile_ccc(&source, &output);

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

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn incomplete_extern_arrays_emit_only_undefined_data_symbols() {
    let directory = test_directory("incomplete-extern-arrays");
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
        let symbol = file
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing `{name}`"));
        assert!(symbol.is_undefined(), "{name}");
        assert!(symbol.is_global(), "{name}");
        assert_eq!(symbol.kind(), SymbolKind::Data, "{name}");
        assert_eq!(symbol.size(), 0, "{name}");
    }

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn declaration_assembly_labels_control_defined_and_referenced_elf_symbols() {
    let directory = test_directory("declaration-assembly-labels");
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
    compile_ccc(&source, &output);

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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[derive(Clone, Debug)]
struct ReferenceCompiler {
    program: OsString,
    arguments: Vec<OsString>,
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
impl ReferenceCompiler {
    fn required() -> Self {
        let value = std::env::var_os("CC").unwrap_or_else(|| OsString::from("cc"));
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
        assert!(output.status.success(), "{}", render_output(&output));
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
        assert!(result.status.success(), "{}", render_output(&result));
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
        assert!(result.status.success(), "{}", render_output(&result));
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn write_source(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn run_successfully(executable: &Path) {
    let result = Command::new(executable).output().unwrap();
    assert!(result.status.success(), "{}", render_output(&result));
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn objects_cross_link_in_both_directions_and_keep_static_names_local() {
    let directory = test_directory("cross-link");
    let reference = ReferenceCompiler::required();
    eprintln!("cross-link reference compiler: {}", reference.identity());

    let ccc_caller = write_source(
        &directory,
        "ccc-caller.c",
        "extern int reference_add(int); int main(void) { return reference_add(35) == 42 ? 0 : 1; }\n",
    );
    let reference_callee = write_source(
        &directory,
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
        &directory,
        "ccc-callee.c",
        "int ccc_multiply(int left, int right) { return left * right; }\n",
    );
    let reference_caller = write_source(
        &directory,
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
        &directory,
        "left.c",
        "static int hidden = 19; int left(void) { return hidden; }\n",
    );
    let right = write_source(
        &directory,
        "right.c",
        "static int hidden = 23; int right(void) { return hidden; }\n",
    );
    let local_main = write_source(
        &directory,
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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn thread_local_objects_cross_link_with_the_platform_compiler() {
    let directory = test_directory("thread-local-cross-link");
    let reference = ReferenceCompiler::required();
    eprintln!("TLS reference compiler: {}", reference.identity());

    let ccc_definition = write_source(
        &directory,
        "ccc-tls-definition.c",
        "_Thread_local int ccc_tls_value = 17;\n\
         int ccc_tls_read(void) { return ccc_tls_value; }\n",
    );
    let reference_consumer = write_source(
        &directory,
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
        &directory,
        "ccc-tls-reader.c",
        "_Thread_local int reference_tls_value __attribute__((weak));\n\
         int reference_tls_read(void) { return reference_tls_value; }\n",
    );
    let reference_definition = write_source(
        &directory,
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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn weak_definitions_link_as_fallbacks_for_strong_symbols() {
    let directory = test_directory("weak-definition-interop");
    let reference = ReferenceCompiler::required();
    eprintln!("weak-symbol reference compiler: {}", reference.identity());

    let weak_source = write_source(
        &directory,
        "weak.c",
        "int selected(void) __attribute__((weak));\n\
         int selected(void) { return 1; }\n\
         int selected_value __attribute__((weak)) = 2;\n\
         int observe_selection(void) { return selected() + selected_value; }\n",
    );
    let fallback_main = write_source(
        &directory,
        "fallback-main.c",
        "extern int observe_selection(void);\n\
         int main(void) { return observe_selection() == 3 ? 0 : 1; }\n",
    );
    let strong_main = write_source(
        &directory,
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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn declaration_assembly_labels_interoperate_in_both_directions() {
    let directory = test_directory("declaration-assembly-label-interop");
    let reference = ReferenceCompiler::required();
    eprintln!(
        "assembly-label reference compiler: {}",
        reference.identity()
    );

    let ccc_caller = write_source(
        &directory,
        "ccc-caller.c",
        r#"
extern int public_add(int) asm("reference_add_impl");
extern int public_value asm("reference_value_impl");
int main(void) {
    return public_add(35) + public_value == 49 ? 0 : 1;
}
"#,
    );
    let reference_callee = write_source(
        &directory,
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

    let ccc_callee = write_source(
        &directory,
        "ccc-callee.c",
        r#"
int internal_multiply(int, int) asm("ccc_multiply_impl");
int internal_multiply(int left, int right) { return left * right; }
int internal_value asm("ccc_value_impl") = 6;
"#,
    );
    let reference_caller = write_source(
        &directory,
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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn glibc_redirect_assembly_labels_form_elf_symbols_that_link() {
    let directory = test_directory("glibc-redirect-labels");
    let reference = ReferenceCompiler::required();
    eprintln!(
        "glibc redirect reference compiler: {}",
        reference.identity()
    );
    let source = write_source(
        &directory,
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
    assert!(
        result.status.success(),
        "CCC failed to compile installed glibc redirects:\n{}",
        render_output(&result)
    );

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

    fs::remove_dir_all(directory).unwrap();
}
