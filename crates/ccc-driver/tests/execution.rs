#![cfg_attr(
    not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "riscv64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "macos")
    )),
    allow(dead_code)
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/execution/cases")
        .join(name)
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn macos_sdk_root() -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("the Darwin execution gate requires xcrun");
    support::assert_command_success("locate the macOS SDK with xcrun", &output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn compile_and_run_darwin_header_program(name: &str, source_text: &str) {
    let directory = support::TestWorkspace::new("execution", name).retain_on_failure();
    let source = directory.join(format!("{name}.c"));
    let executable = directory.join(name);
    fs::write(&source, source_text).unwrap();
    let compilation = support::ccc_command()
        .arg("--target=aarch64-apple-darwin")
        .args(["--sdk-root", &macos_sdk_root()])
        .arg("-mmacosx-version-min=11.0")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("compile a Darwin header program with CCC", &compilation);
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(0),
        "program failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn host_default_emits_a_native_relocatable_object() {
    use object::{Architecture, Object as _, ObjectKind};

    let directory = support::TestWorkspace::new("execution", "empty-object").retain_on_failure();
    let output = directory.join("empty.o");
    let result = support::ccc_command()
        .arg("-c")
        .arg(fixture("empty.c"))
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    directory.assert_command_success("compile an empty native object with CCC", &result);
    let bytes = fs::read(&output).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    let expected_architecture = if cfg!(target_arch = "x86_64") {
        Architecture::X86_64
    } else if cfg!(target_arch = "aarch64") {
        Architecture::Aarch64
    } else {
        Architecture::Riscv64
    };
    assert_eq!(object.architecture(), expected_architecture);
    assert_eq!(object.kind(), ObjectKind::Relocatable);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn float16_values_execute_with_exact_payloads_and_native_varargs() {
    let directory = support::TestWorkspace::new("execution", "float16-values").retain_on_failure();
    let executable = directory.join("float16-values");
    let compilation = support::ccc_command()
        .arg(fixture("float16_values.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success(
        "compile the Float16 execution fixture with CCC",
        &compilation,
    );
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(execution.status.code(), Some(0));
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn selected_c11_results_match_the_host_compiler() {
    let directory =
        support::TestWorkspace::new("execution", "selected-c11-differential").retain_on_failure();
    let reference_driver = std::env::var_os("CCC_REFERENCE_CC")
        .or_else(|| std::env::var_os("CCC_CC"))
        .unwrap_or_else(|| "cc".into());
    for source_name in [
        "generic_selection.c",
        "compound_literals.c",
        "runtime_sized_storage.c",
    ] {
        let source = fixture(source_name);
        let ccc_executable = directory.join(format!("{source_name}-ccc"));
        let reference_executable = directory.join(format!("{source_name}-reference"));

        let ccc_compilation = support::ccc_command()
            .arg(&source)
            .arg("-o")
            .arg(&ccc_executable)
            .output()
            .unwrap();
        directory
            .assert_command_success(&format!("compile {source_name} with CCC"), &ccc_compilation);
        let reference_compilation = Command::new(&reference_driver)
            .args(["-std=c11", "-pedantic-errors"])
            .arg(&source)
            .arg("-o")
            .arg(&reference_executable)
            .output()
            .unwrap();
        directory.assert_command_success(
            &format!("compile {source_name} with the reference compiler"),
            &reference_compilation,
        );

        let ccc_result = Command::new(&ccc_executable).output().unwrap();
        let reference_result = Command::new(&reference_executable).output().unwrap();
        assert_eq!(
            ccc_result.status.code(),
            reference_result.status.code(),
            "exit status differs for {source_name}"
        );
        assert_eq!(ccc_result.stdout, reference_result.stdout, "{source_name}");
        assert_eq!(ccc_result.stderr, reference_result.stderr, "{source_name}");
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn darwin_linker_accepts_unwind_when_functions_reference_constant_data() {
    let directory = support::TestWorkspace::new("execution", "darwin-text-before-data-unwind")
        .retain_on_failure();
    let source = directory.join("darwin-text-before-data-unwind.c");
    let executable = directory.join("darwin-text-before-data-unwind");
    fs::write(
        &source,
        "int first(void) { return \"x\"[0]; }\n\
         int main(void) { return first() == 'x' ? 0 : 1; }\n",
    )
    .unwrap();

    let compilation = support::ccc_command()
        .arg("--target=aarch64-apple-darwin")
        .arg("-nostdinc")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("compile the Darwin unwind fixture with CCC", &compilation);
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(execution.status.code(), Some(0));
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn apple_sdk_redirects_and_header_inline_fallback_link_and_execute() {
    compile_and_run_darwin_header_program(
        "darwin-sdk-redirects",
        "#include <ctype.h>\n\
         #include <stdio.h>\n\
         int main(void) {\n\
             FILE *stream = fopen(\"/dev/null\", \"wb\");\n\
             if (!stream) return 1;\n\
             if (!isalpha('A') || !iscntrl('\\n')) return 2;\n\
             if (putc_unlocked('x', stream) < 0) return 3;\n\
             if (fwrite(\"ok\", 1, 2, stream) != 2) return 4;\n\
             return fclose(stream) != 0;\n\
         }\n",
    );
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn darwin_setjmp_and_longjmp_resume_materialized_automatic_objects() {
    let directory =
        support::TestWorkspace::new("execution", "darwin-returns-twice").retain_on_failure();
    for optimization in ["-O0", "-O2", "-Oz"] {
        let executable = directory.join(format!("returns-twice-{}", &optimization[1..]));
        let compilation = support::ccc_command()
            .arg("--target=aarch64-apple-darwin")
            .args(["--sdk-root", &macos_sdk_root()])
            .arg("-mmacosx-version-min=11.0")
            .arg(optimization)
            .arg(fixture("returns_twice.c"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        directory.assert_command_success(
            &format!("compile the Darwin returns-twice fixture under {optimization}"),
            &compilation,
        );
        assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn linux_setjmp_and_longjmp_resume_materialized_automatic_objects() {
    let directory =
        support::TestWorkspace::new("execution", "linux-returns-twice").retain_on_failure();
    for optimization in ["-O0", "-O2", "-Oz"] {
        let executable = directory.join(format!("returns-twice-{}", &optimization[1..]));
        let compilation = support::ccc_command()
            .arg(optimization)
            .arg(fixture("returns_twice.c"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        directory.assert_command_success(
            &format!("compile the Linux returns-twice fixture under {optimization}"),
            &compilation,
        );
        assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn apple_math_public_classifiers_evaluate_once_and_fabsl_stays_a_library_call() {
    compile_and_run_darwin_header_program(
        "darwin-math-wrapper",
        "#include <math.h>\n\
         static int evaluations;\n\
         static double once(double value) { ++evaluations; return value; }\n\
         static long double once_long(long double value) { ++evaluations; return value; }\n\
         int main(void) {\n\
             evaluations = 0;\n\
             if (!isfinite(once(1.0)) || evaluations != 1) return 1;\n\
             evaluations = 0;\n\
             if (!isinf(once(INFINITY)) || evaluations != 1) return 2;\n\
             evaluations = 0;\n\
             if (!isnan(once(NAN)) || evaluations != 1) return 3;\n\
             evaluations = 0;\n\
             if (!isfinite(once_long(1.0L)) || evaluations != 1) return 4;\n\
             if (fabsl(-2.0L) != 2.0L) return 5;\n\
             return 0;\n\
         }\n",
    );
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn apple_variadic_bridge_uses_the_exact_declaration_assembly_label() {
    compile_and_run_darwin_header_program(
        "darwin-exact-variadic-label",
        "#include <stdarg.h>\n\
         int source_sum(int count, ...) asm(\"_physical_sum\");\n\
         int source_sum(int count, ...) {\n\
             va_list list;\n\
             int total = 0;\n\
             int index;\n\
             va_start(list, count);\n\
             for (index = 0; index < count; ++index) total += va_arg(list, int);\n\
             va_end(list);\n\
             return total;\n\
         }\n\
         int main(void) { return source_sum(3, 4, 5, 6) != 15; }\n",
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn execution_programs_emit_native_objects() {
    use object::{Object as _, ObjectSymbol as _};

    for case in execution_cases() {
        if !case.applies_to_native_host() {
            continue;
        }
        let name = case.source;
        let directory = support::TestWorkspace::new("execution-objects", name).retain_on_failure();
        for (optimization, artifact) in EXECUTION_OPTIMIZATION_PROFILES {
            let output = directory.join(format!("program-{artifact}.o"));
            let result = support::ccc_command()
                .arg(format!("--target={}", native_target_triple()))
                .arg(optimization)
                .arg("-c")
                .arg(fixture(name))
                .arg("-o")
                .arg(&output)
                .output()
                .unwrap();
            directory.assert_command_success(
                &format!("compile {name} to an object under {optimization}"),
                &result,
            );
            let bytes = fs::read(&output).unwrap();
            let object = object::File::parse(bytes.as_slice()).unwrap();
            assert_eq!(
                object.architecture(),
                native_object_architecture(),
                "{name} under {optimization}"
            );
            assert!(
                object
                    .symbols()
                    .any(|symbol| symbol.name() == Ok(native_main_symbol())),
                "{name} under {optimization} has no main symbol"
            );
        }
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn execution_programs_produce_the_expected_exit_status() {
    for case in execution_cases() {
        if !case.applies_to_native_host() {
            continue;
        }
        let name = case.source;
        let directory = support::TestWorkspace::new("execution-programs", name).retain_on_failure();
        for (optimization, artifact) in EXECUTION_OPTIMIZATION_PROFILES {
            let executable = directory.join(format!("program-{artifact}"));
            let compilation = support::ccc_command()
                .arg(format!("--target={}", native_target_triple()))
                .arg(optimization)
                .arg(fixture(name))
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            directory.assert_command_success(
                &format!("compile and link {name} under {optimization}"),
                &compilation,
            );
            let execution = Command::new(&executable)
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            assert_eq!(
                execution.status.code(),
                Some(case.status),
                "wrong exit status for {name} under {optimization}; stderr: {}",
                String::from_utf8_lossy(&execution.stderr)
            );
            assert_eq!(
                execution.stdout,
                case.stdout,
                "wrong stdout for {name} under {optimization}: {}",
                String::from_utf8_lossy(&execution.stdout)
            );
            assert_eq!(
                execution.stderr,
                case.stderr,
                "wrong stderr for {name} under {optimization}: {}",
                String::from_utf8_lossy(&execution.stderr)
            );
        }
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn default_link_produces_a_working_position_independent_executable() {
    use object::{FileFlags, Object as _, ObjectKind, ObjectSection as _};

    let directory = support::TestWorkspace::new("execution", "position-independent-executable")
        .retain_on_failure();
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

    let compilation = support::ccc_command()
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("compile and link a native PIE with CCC", &compilation);

    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    if cfg!(target_os = "linux") {
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
    } else {
        assert_eq!(file.kind(), ObjectKind::Executable);
        let FileFlags::MachO { flags } = file.flags() else {
            panic!("Darwin executable did not carry Mach-O flags");
        };
        assert_ne!(flags & object::macho::MH_PIE, 0, "Mach-O output is not PIE");
    }

    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(0),
        "PIE failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

// GNU __int128 values and runtime-provider boundaries are currently enabled
// only for the System V AMD64 profile. The other execution suites still run.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn wide_integer_runtime_helpers_resolve_through_the_ccc_link_path() {
    let directory =
        support::TestWorkspace::new("execution", "wide-runtime-provider").retain_on_failure();
    let source = directory.join("wide-runtime-provider.c");
    fs::write(
        &source,
        r#"
typedef __int128 i128;

static i128 divide(i128 left, i128 right) { return left / right; }
static i128 remainder(i128 left, i128 right) { return left % right; }
static double to_double(i128 value) { return (double)value; }
static i128 from_double(double value) { return (i128)value; }

int main(void) {
    i128 value = ((i128)1 << 100) + 12345;
    i128 divisor = 97;
    i128 quotient = divide(value, divisor);
    i128 residual = remainder(value, divisor);
    if (quotient * divisor + residual != value) return 1;
    if (from_double(to_double((i128)1 << 100)) != ((i128)1 << 100)) return 2;
    return 0;
}
"#,
    )
    .unwrap();

    for compiler in ["gcc", "clang"] {
        let executable = directory.join(format!("program-{compiler}"));
        let compilation = support::ccc_command()
            .env("CCC_CC", compiler)
            .arg("--target=x86_64-unknown-linux-gnu")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        directory.assert_command_success(
            &format!("compile and link the wide runtime fixture through {compiler}"),
            &compilation,
        );
        let execution = Command::new(&executable).output().unwrap();
        directory.assert_command_success(
            &format!("run the wide runtime fixture linked through {compiler}"),
            &execution,
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
fn thread_local_objects_are_isolated_in_pthreads_and_pie() {
    use object::{Object as _, ObjectKind};

    let directory =
        support::TestWorkspace::new("execution", "thread-local-pthreads").retain_on_failure();
    let executable = directory.join("thread-local-pthreads");
    let compilation = support::ccc_command()
        .arg(fixture("thread_local_pthreads.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("compile and link the pthread TLS fixture", &compilation);
    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            file.kind(),
            ObjectKind::Dynamic,
            "default ELF output must be PIE"
        );
    } else {
        assert_eq!(
            file.kind(),
            ObjectKind::Executable,
            "default Mach-O output must be an executable"
        );
    }

    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(66),
        "pthread TLS fixture failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn all_tls_models_link_and_execute_as_pie() {
    use object::{Object as _, ObjectKind};

    let directory =
        support::TestWorkspace::new("execution", "thread-local-models-pie").retain_on_failure();
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
    let compilation = support::ccc_command()
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("compile and link the TLS-model PIE fixture", &compilation);
    let bytes = fs::read(&executable).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            file.kind(),
            ObjectKind::Dynamic,
            "default ELF output must be PIE"
        );
    } else {
        assert_eq!(
            file.kind(),
            ObjectKind::Executable,
            "default Mach-O output must be an executable"
        );
    }
    let execution = Command::new(&executable).output().unwrap();
    directory.assert_command_success("run the TLS-model PIE fixture", &execution);
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn an_invalid_computed_goto_target_traps() {
    use std::os::unix::process::ExitStatusExt as _;

    let directory =
        support::TestWorkspace::new("execution", "computed-goto-null").retain_on_failure();
    let executable = directory.join("program");
    let compilation = support::ccc_command()
        .arg(fixture("computed_goto_null.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success(
        "compile and link the invalid computed-goto fixture",
        &compilation,
    );
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(execution.status.code(), None);
    assert!(
        matches!(execution.status.signal(), Some(4 | 5)),
        "expected SIGILL or SIGTRAP, got {:?}",
        execution.status.signal()
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn runtime_sized_aggregate_return_is_materialized_before_cleanup() {
    let directory = support::TestWorkspace::new("execution", "runtime-sized-aggregate-return")
        .retain_on_failure();
    let executable = directory.join("program");
    let compilation = support::ccc_command()
        .arg(fixture("runtime_sized_storage_reuse.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success(
        "compile and link the runtime-sized aggregate return fixture",
        &compilation,
    );
    let execution = Command::new(&executable).output().unwrap();
    assert_eq!(
        execution.status.code(),
        Some(66),
        "runtime-sized aggregate return failed: {}",
        String::from_utf8_lossy(&execution.stderr)
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn invalid_runtime_sized_storage_extents_trap() {
    use std::os::unix::process::ExitStatusExt as _;

    let mut names = vec![
        "runtime_sized_storage_nonpositive.c",
        "runtime_sized_storage_overflow.c",
        "runtime_sized_storage_provider_failure.c",
    ];
    if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        names.push("runtime_sized_storage_wide_overflow.c");
    }
    for name in names {
        let directory = support::TestWorkspace::new("execution-invalid-runtime-sized", name)
            .retain_on_failure();
        let executable = directory.join("program");
        let compilation = support::ccc_command()
            .arg(fixture(name))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        directory.assert_command_success(
            &format!("compile and link invalid runtime-sized fixture {name}"),
            &compilation,
        );
        let execution = Command::new(&executable).output().unwrap();
        assert_eq!(execution.status.code(), None, "{name}");
        assert!(
            matches!(execution.status.signal(), Some(4 | 5)),
            "{name}: expected SIGILL or SIGTRAP, got {:?}",
            execution.status.signal()
        );
    }
}

// This is the independently provisioned GCC ASan/LSan provider gate, not a
// restriction on the portable runtime-sized-storage execution cases above.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn runtime_sized_storage_links_with_gcc_as_pie_and_has_no_leaks() {
    let directory = support::TestWorkspace::new("execution", "runtime-sized-storage-external-link")
        .retain_on_failure();
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

    let compilation = support::ccc_command()
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    directory.assert_command_success("compile runtime-sized storage with CCC", &compilation);
    let link = Command::new("gcc")
        .arg("-fsanitize=address")
        .arg(&object)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    directory.assert_command_success("link runtime-sized storage with GCC and ASan", &link);
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
}

struct ExecutionExpectation {
    source: &'static str,
    status: i32,
    stdout: &'static [u8],
    stderr: &'static [u8],
    x86_64_only: bool,
}

impl ExecutionExpectation {
    fn applies_to_native_host(&self) -> bool {
        !self.x86_64_only || cfg!(target_arch = "x86_64")
    }
}

const fn exit_status(source: &'static str, status: i32) -> ExecutionExpectation {
    ExecutionExpectation {
        source,
        status,
        stdout: b"",
        stderr: b"",
        x86_64_only: false,
    }
}

const fn x86_64_exit_status(source: &'static str, status: i32) -> ExecutionExpectation {
    ExecutionExpectation {
        source,
        status,
        stdout: b"",
        stderr: b"",
        x86_64_only: true,
    }
}

fn execution_cases() -> &'static [ExecutionExpectation] {
    &EXECUTION_CASES
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
const fn native_target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
const fn native_target_triple() -> &'static str {
    "aarch64-unknown-linux-gnu"
}

#[cfg(all(target_arch = "riscv64", target_os = "linux"))]
const fn native_target_triple() -> &'static str {
    "riscv64-unknown-linux-gnu"
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
const fn native_target_triple() -> &'static str {
    "aarch64-apple-darwin"
}

#[cfg(target_arch = "x86_64")]
const fn native_object_architecture() -> object::Architecture {
    object::Architecture::X86_64
}

#[cfg(target_arch = "aarch64")]
const fn native_object_architecture() -> object::Architecture {
    object::Architecture::Aarch64
}

#[cfg(target_arch = "riscv64")]
const fn native_object_architecture() -> object::Architecture {
    object::Architecture::Riscv64
}

#[cfg(target_os = "macos")]
const fn native_main_symbol() -> &'static str {
    "_main"
}

#[cfg(target_os = "linux")]
const fn native_main_symbol() -> &'static str {
    "main"
}

const EXECUTION_OPTIMIZATION_PROFILES: [(&str, &str); 3] =
    [("-O0", "o0"), ("-O2", "o2"), ("-Oz", "oz")];

static EXECUTION_CASES: [ExecutionExpectation; 59] = [
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
    exit_status("generic_selection.c", 44),
    exit_status("flexible_array_members.c", 59),
    exit_status("sync_atomic_builtins.c", 60),
    exit_status("sync_atomic_pthreads.c", 64),
    exit_status("c11_atomic_builtins.c", 68),
    exit_status("thread_local_pthreads.c", 66),
    exit_status("integer_intrinsics.c", 61),
    exit_status("predefined_function_name.c", 62),
    exit_status("alignment_and_transparent_union.c", 65),
    exit_status("aligned_integer_typedefs.c", 67),
    exit_status("runtime_sized_storage.c", 0),
    exit_status("runtime_sized_storage_reuse.c", 66),
    exit_status("negative_switch_constant.c", 0),
    exit_status("gnu_statement_and_memory_builtins.c", 66),
    x86_64_exit_status("inline_assembly.c", 0),
    exit_status("combined_language_features.c", 53),
    exit_status("semantic_regressions.c", 54),
    exit_status("aggregate_calls.c", 63),
    exit_status("aggregate_rvalue_arrays.c", 42),
    exit_status("aggregate_rvalue_bitfield.c", 37),
    exit_status("variadic_functions.c", 93),
    ExecutionExpectation {
        source: "variadic_printf.c",
        status: 0,
        stdout: b"ccc 7 2.5 ok\n",
        stderr: b"",
        x86_64_only: false,
    },
];
