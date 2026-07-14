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

    for (name, _) in execution_cases() {
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
    for (name, expected) in execution_cases() {
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
        assert_eq!(
            execution.status.code(),
            Some(*expected),
            "wrong exit status for {name}; stderr: {}",
            String::from_utf8_lossy(&execution.stderr)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

fn execution_cases() -> &'static [(&'static str, i32)] {
    &[
        ("return_constant.c", 42),
        ("arithmetic_precedence.c", 14),
        ("unary_arithmetic.c", 3),
        ("local_initializers.c", 42),
        ("assignment.c", 42),
        ("nested_scope.c", 7),
        ("if_else.c", 11),
        ("nested_conditionals.c", 25),
        ("while_loop.c", 10),
        ("while_assignment.c", 6),
        ("comparisons.c", 63),
        ("short_circuit.c", 40),
        ("call_no_arguments.c", 17),
        ("call_with_arguments.c", 42),
        ("recursion.c", 120),
        ("forward_declaration.c", 42),
        ("external_call.c", 42),
        ("main_fallthrough.c", 0),
        ("unused_fallthrough_result.c", 7),
        ("minimum_signed_int.c", 1),
        ("header_program.c", 42),
        ("integer_types.c", 41),
        ("pointers_and_arrays.c", 42),
        ("records_unions_enums.c", 43),
        ("bitfields_and_packing.c", 44),
        ("globals_and_static_initializers.c", 45),
        ("string_literals.c", 46),
        ("full_control_flow.c", 47),
        ("indirect_calls.c", 48),
        ("layout_operators.c", 49),
        ("volatile_access.c", 50),
        ("floating_point.c", 51),
        ("operators_and_conversions.c", 52),
        ("combined_language_features.c", 53),
        ("semantic_regressions.c", 54),
    ]
}
