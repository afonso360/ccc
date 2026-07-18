use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ccc-execution-test-{}-{}-{name}",
        std::process::id(),
        TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/execution/cases")
        .join(name)
}

#[test]
fn empty_translation_unit_emits_a_valid_object() {
    let directory = test_directory("empty-object");
    let output = directory.join("empty.o");
    let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-c")
        .arg(fixture("empty.c"))
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let object = fs::read(&output).unwrap();
    assert!(ccc_driver::is_empty_elf64_relocatable(&object));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn execution_programs_emit_x86_64_objects() {
    use object::{Architecture, Object as _, ObjectSymbol as _};

    for case in execution_cases() {
        if case.requires_bridge && !bridge_packaging_is_available() {
            continue;
        }
        let name = case.source;
        let directory = test_directory(name);
        let output = directory.join("program.o");
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .arg("-c")
            .arg(fixture(name))
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "ccc failed for {name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        assert_eq!(object.architecture(), Architecture::X86_64, "{name}");
        assert!(
            object.symbols().any(|symbol| symbol.name() == Ok("main")),
            "{name} has no main symbol"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn execution_programs_produce_the_expected_exit_status() {
    for case in execution_cases() {
        let name = case.source;
        let directory = test_directory(name);
        let executable = directory.join("program");
        let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .arg(fixture(name))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "ccc failed for {name}: {}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let execution = Command::new(&executable)
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert_eq!(
            execution.status.code(),
            Some(case.status),
            "wrong exit status for {name}; stderr: {}",
            String::from_utf8_lossy(&execution.stderr)
        );
        assert_eq!(
            execution.stdout,
            case.stdout,
            "wrong stdout for {name}: {}",
            String::from_utf8_lossy(&execution.stdout)
        );
        assert_eq!(
            execution.stderr,
            case.stderr,
            "wrong stderr for {name}: {}",
            String::from_utf8_lossy(&execution.stderr)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn default_link_produces_a_working_position_independent_executable() {
    use object::{Object as _, ObjectKind, ObjectSection as _};

    let directory = test_directory("position-independent-executable");
    let source = directory.join("position-independent-executable.c");
    let executable = directory.join("position-independent-executable");
    fs::write(
        &source,
        r#"
int stored_value = 35;
int *stored_pointer = &stored_value;

int add_seven(int value) {
    return value + 7;
}

int (*stored_function)(int) = add_seven;

int main(void) {
    return stored_function(*stored_pointer) == 42 ? 0 : 1;
}
"#,
    )
    .unwrap();

    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );

    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    assert_eq!(file.kind(), ObjectKind::Dynamic);
    let text = file.section_by_name(".text").unwrap();
    let text_start = text.address();
    let text_end = text_start + text.size();
    assert!(
        file.dynamic_relocations()
            .unwrap()
            .all(|(address, _)| address < text_start || address >= text_end),
        "PIE has a dynamic relocation in executable text"
    );

    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(0),
        "PIE failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn thread_local_objects_are_isolated_in_pthreads_and_pie() {
    use object::{Object as _, ObjectKind};

    let directory = test_directory("thread-local-pthreads");
    let executable = directory.join("thread-local-pthreads");
    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg(fixture("thread_local_pthreads.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    assert_eq!(
        file.kind(),
        ObjectKind::Dynamic,
        "default output must be PIE"
    );

    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(66),
        "pthread TLS fixture failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn all_elf_tls_models_link_and_execute_as_pie() {
    use object::{Object as _, ObjectKind};

    let directory = test_directory("thread-local-models-pie");
    let source = directory.join("thread-local-models-pie.c");
    let executable = directory.join("thread-local-models-pie");
    fs::write(
        &source,
        r#"
_Thread_local int gd __attribute__((tls_model("global-dynamic"))) = 3;
_Thread_local int ld __attribute__((tls_model("local-dynamic"))) = 5;
_Thread_local int ie __attribute__((tls_model("initial-exec"))) = 7;
_Thread_local int le __attribute__((tls_model("local-exec"))) = 11;

int main(void) {
    if (gd + ld + ie + le != 26) return 1;
    gd = 13;
    ld = 17;
    ie = 19;
    le = 23;
    return gd + ld + ie + le == 72 ? 0 : 2;
}
"#,
    )
    .unwrap();
    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    assert_eq!(
        file.kind(),
        ObjectKind::Dynamic,
        "default output must be PIE"
    );
    let execution = Command::new(&executable).output().unwrap();
    assert!(
        execution.status.success(),
        "TLS model PIE failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn an_invalid_computed_goto_target_traps() {
    use std::os::unix::process::ExitStatusExt as _;

    let directory = test_directory("computed-goto-null");
    let executable = directory.join("program");
    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg(fixture("computed_goto_null.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(execution.status.code(), None);
    assert_eq!(execution.status.signal(), Some(4));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn runtime_sized_aggregate_return_is_materialized_before_cleanup() {
    let directory = test_directory("runtime-sized-aggregate-return");
    let executable = directory.join("program");
    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg(fixture("runtime_sized_storage_reuse.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(66),
        "runtime-sized aggregate return failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn invalid_runtime_sized_storage_extents_trap() {
    use std::os::unix::process::ExitStatusExt as _;

    for name in [
        "runtime_sized_storage_nonpositive.c",
        "runtime_sized_storage_overflow.c",
    ] {
        let directory = test_directory(name);
        let executable = directory.join("program");
        let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .arg(fixture(name))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "ccc failed for {name}: {}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let execution = Command::new(&executable).output().unwrap();
        assert_eq!(execution.status.code(), None, "{name}");
        assert_eq!(execution.status.signal(), Some(4), "{name}");
        fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn runtime_sized_storage_links_with_gcc_as_pie_and_has_no_leaks() {
    let directory = test_directory("runtime-sized-storage-external-link");
    let source = directory.join("runtime-sized-storage-external-link.c");
    let object = directory.join("runtime-sized-storage-external-link.o");
    let executable = directory.join("runtime-sized-storage-external-link");
    fs::write(
        &source,
        r#"
static int exercise(int extent) {
    _Alignas(64) int values[extent];
    values[extent - 1] = extent;
    return values[extent - 1] != extent;
}

int main(void) {
    for (int extent = 1; extent <= 128; ++extent) {
        if (exercise(extent) != 0) return 1;
    }
    return 0;
}
"#,
    )
    .unwrap();

    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let link = Command::new("gcc")
        .arg("-fsanitize=address")
        .arg(&object)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        link.status.success(),
        "gcc failed: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    let execution = Command::new(&executable)
        .env("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1:exitcode=97")
        .output()
        .unwrap();
    assert_eq!(
        execution.status.code(),
        Some(0),
        "sanitized PIE failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg_attr(
    not(all(target_arch = "x86_64", target_os = "linux")),
    allow(dead_code)
)]
struct ExecutionExpectation {
    source: &'static str,
    status: i32,
    stdout: &'static [u8],
    stderr: &'static [u8],
    requires_bridge: bool,
}

const fn exit_status(source: &'static str, status: i32) -> ExecutionExpectation {
    ExecutionExpectation {
        source,
        status,
        stdout: b"",
        stderr: b"",
        requires_bridge: false,
    }
}

const fn bridged_exit_status(source: &'static str, status: i32) -> ExecutionExpectation {
    ExecutionExpectation {
        source,
        status,
        stdout: b"",
        stderr: b"",
        requires_bridge: true,
    }
}

fn bridge_packaging_is_available() -> bool {
    cfg!(all(target_arch = "x86_64", target_os = "linux")) || std::env::var_os("CCC_CC").is_some()
}

fn execution_cases() -> &'static [ExecutionExpectation] {
    &EXECUTION_CASES
}

static EXECUTION_CASES: [ExecutionExpectation; 53] = [
    exit_status("return_constant.c", 42),
    exit_status("arithmetic_precedence.c", 14),
    exit_status("unary_arithmetic.c", 3),
    exit_status("local_initializers.c", 42),
    exit_status("assignment.c", 42),
    exit_status("nested_scope.c", 7),
    exit_status("if_else.c", 11),
    exit_status("nested_conditionals.c", 25),
    exit_status("while_loop.c", 10),
    exit_status("while_assignment.c", 6),
    exit_status("comparisons.c", 63),
    exit_status("short_circuit.c", 40),
    exit_status("call_no_arguments.c", 17),
    exit_status("call_with_arguments.c", 42),
    exit_status("recursion.c", 120),
    exit_status("forward_declaration.c", 42),
    exit_status("external_call.c", 42),
    exit_status("main_fallthrough.c", 0),
    exit_status("unused_fallthrough_result.c", 7),
    exit_status("minimum_signed_int.c", 1),
    exit_status("header_program.c", 42),
    exit_status("integer_types.c", 41),
    exit_status("pointers_and_arrays.c", 42),
    exit_status("records_unions_enums.c", 43),
    exit_status("bitfields_and_packing.c", 44),
    exit_status("globals_and_static_initializers.c", 45),
    exit_status("string_literals.c", 46),
    exit_status("full_control_flow.c", 47),
    exit_status("indirect_calls.c", 48),
    exit_status("layout_operators.c", 49),
    exit_status("volatile_access.c", 50),
    exit_status("floating_point.c", 51),
    exit_status("operators_and_conversions.c", 52),
    exit_status("integer_to_pointer.c", 55),
    exit_status("scalar_builtins.c", 56),
    exit_status("computed_goto.c", 57),
    exit_status("compound_literals.c", 58),
    exit_status("flexible_array_members.c", 59),
    exit_status("sync_atomic_builtins.c", 60),
    exit_status("sync_atomic_pthreads.c", 64),
    bridged_exit_status("thread_local_pthreads.c", 66),
    exit_status("integer_intrinsics.c", 61),
    exit_status("predefined_function_name.c", 62),
    exit_status("alignment_and_transparent_union.c", 65),
    exit_status("runtime_sized_storage.c", 0),
    exit_status("runtime_sized_storage_reuse.c", 66),
    exit_status("combined_language_features.c", 53),
    exit_status("semantic_regressions.c", 54),
    exit_status("aggregate_calls.c", 63),
    exit_status("aggregate_rvalue_arrays.c", 42),
    exit_status("aggregate_rvalue_bitfield.c", 37),
    bridged_exit_status("variadic_functions.c", 93),
    ExecutionExpectation {
        source: "variadic_printf.c",
        status: 0,
        stdout: b"ccc 7 2.5 ok\n",
        stderr: b"",
        requires_bridge: true,
    },
];
