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
    let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-nostdinc")
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "CCC failed:\n{}",
        render_output(&result)
    );
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
            .arg("-no-pie")
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
