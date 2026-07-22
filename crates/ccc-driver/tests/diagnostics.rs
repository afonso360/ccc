use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Case {
    name: &'static str,
    expected: &'static str,
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temporary_directory(name: &str) -> PathBuf {
    let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "ccc-diagnostics-{}-{serial}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn rejected_translations_match_diagnostic_goldens_and_emit_no_object() {
    let cases = [
        Case {
            name: "linkage-object",
            expected: include_str!("../../../tests/diagnostics/goldens/linkage-object.stderr"),
        },
        Case {
            name: "linkage-function",
            expected: include_str!("../../../tests/diagnostics/goldens/linkage-function.stderr"),
        },
        Case {
            name: "undeclared-identifier",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/undeclared-identifier.stderr"
            ),
        },
        Case {
            name: "flexible-array-member",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/flexible-array-member.stderr"
            ),
        },
        Case {
            name: "variable-length-object",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/variable-length-object.stderr"
            ),
        },
        Case {
            name: "variable-length-star",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/variable-length-star.stderr"
            ),
        },
        Case {
            name: "invalid-va-start",
            expected: include_str!("../../../tests/diagnostics/goldens/invalid-va-start.stderr"),
        },
        Case {
            name: "wrong-va-start-parameter",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/wrong-va-start-parameter.stderr"
            ),
        },
        Case {
            name: "nonmodifiable-va-list",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/nonmodifiable-va-list.stderr"
            ),
        },
        Case {
            name: "unaddressable-va-list",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/unaddressable-va-list.stderr"
            ),
        },
        Case {
            name: "promotion-invalid-va-arg",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/promotion-invalid-va-arg.stderr"
            ),
        },
        Case {
            name: "variably-modified-va-arg",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/variably-modified-va-arg.stderr"
            ),
        },
        Case {
            name: "wrong-call-arity",
            expected: include_str!("../../../tests/diagnostics/goldens/wrong-call-arity.stderr"),
        },
        Case {
            name: "assembly-label",
            expected: include_str!("../../../tests/diagnostics/goldens/assembly-label.stderr"),
        },
        Case {
            name: "gnu-thread-local",
            expected: include_str!("../../../tests/diagnostics/goldens/gnu-thread-local.stderr"),
        },
    ];
    let repository = repository();

    for case in cases {
        let directory = temporary_directory(case.name);
        let output = directory.join(format!("{}.o", case.name));
        let input = format!("tests/diagnostics/cases/{}.c", case.name);
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .current_dir(&repository)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args([
                "--target=x86_64-unknown-linux-gnu",
                "-nostdinc",
                "-c",
                &input,
                "-o",
            ])
            .arg(&output)
            .output()
            .unwrap();

        assert!(
            !result.status.success(),
            "{} unexpectedly compiled successfully",
            case.name
        );
        assert!(
            result.stdout.is_empty(),
            "{} wrote stdout:\n{}",
            case.name,
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            String::from_utf8(result.stderr).unwrap(),
            case.expected,
            "{}",
            case.name
        );
        assert!(
            !output.exists(),
            "{} emitted an object despite its diagnostic",
            case.name
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn binary128_long_double_rejections_match_diagnostic_goldens() {
    let cases = [
        Case {
            name: "long-double-operation",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/long-double-operation.stderr"
            ),
        },
        Case {
            name: "long-double-boundary",
            expected: include_str!(
                "../../../tests/diagnostics/goldens/long-double-boundary.stderr"
            ),
        },
    ];
    let repository = repository();

    for case in cases {
        let directory = temporary_directory(case.name);
        let output = directory.join(format!("{}.o", case.name));
        let input = format!("tests/diagnostics/cases/{}.c", case.name);
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .current_dir(&repository)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args([
                "--target=aarch64-unknown-linux-gnu",
                "-nostdinc",
                "-c",
                &input,
                "-o",
            ])
            .arg(&output)
            .output()
            .unwrap();

        assert!(
            !result.status.success(),
            "{} unexpectedly compiled successfully",
            case.name
        );
        assert!(
            result.stdout.is_empty(),
            "{} wrote stdout:\n{}",
            case.name,
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            String::from_utf8(result.stderr).unwrap(),
            case.expected,
            "{}",
            case.name
        );
        assert!(
            !output.exists(),
            "{} emitted an object despite its diagnostic",
            case.name
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn inline_assembly_near_misses_fail_closed_before_object_emission() {
    let cases = [
        ("unknown-template", "void f(void) { asm(\"pause\"); }"),
        (
            "unsupported-alignment",
            "void f(void) { asm(\".p2align 7\"); }",
        ),
        (
            "incomplete-cpuid-clobbers",
            "void f(unsigned value) { asm(\"cpuid\" : \"=a\"(value) : \"a\"(0) : \"ebx\", \"ecx\"); }",
        ),
        (
            "incomplete-rdtsc-outputs",
            "void f(unsigned value) { asm volatile(\"rdtsc\" : \"=a\"(value)); }",
        ),
        (
            "wrong-atomic-width",
            "void f(long *field, int value) { asm volatile(\"lock; xchgq %0, %1\" : \"+q\"(value), \"+m\"(*field)); }",
        ),
        (
            "asm-goto",
            "void f(void) { asm goto(\"\" : : : : target); target: ; }",
        ),
        (
            "symbolic-operand",
            "void f(unsigned value) { asm(\"\" : [value] \"+r\"(value)); }",
        ),
    ];

    for (name, text) in cases {
        let directory = temporary_directory(name);
        let source = directory.join(format!("{name}.c"));
        let object = directory.join(format!("{name}.o"));
        fs::write(&source, text).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .arg("-c")
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{name} unexpectedly compiled");
        assert!(stderr.contains("CCC2454"), "{name}: {stderr}");
        assert!(!object.exists(), "{name} emitted an object after rejection");
        fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "macos")
))]
#[test]
fn float16_value_paths_publish_objects() {
    for (name, source) in [
        ("initialization", "_Float16 value = 1.0;\n"),
        (
            "definition",
            "_Float16 defined(_Float16 value) { return value; }\n",
        ),
        (
            "call",
            "extern _Float16 operation(_Float16); int call(void) { return operation(1.0) != 0; }\n",
        ),
        (
            "arithmetic",
            "int arithmetic(void) { _Float16 value; return value + value != 0; }\n",
        ),
        (
            "va-arg",
            "typedef __builtin_va_list va_list; int read(int count, ...) { va_list list; __builtin_va_start(list, count); return __builtin_va_arg(list, _Float16) != 0; }\n",
        ),
    ] {
        let directory = temporary_directory(name);
        let input = directory.join(format!("float16-{name}.c"));
        let output = directory.join(format!("float16-{name}.o"));
        fs::write(&input, source).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args(["-nostdinc", "-c"])
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();

        assert!(
            result.status.success(),
            "{name} did not compile: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty(), "{name} wrote stdout");
        assert!(result.stderr.is_empty(), "{name} wrote stderr");
        assert!(output.exists(), "{name} did not emit an object");
        fs::remove_dir_all(directory).unwrap();
    }
}
