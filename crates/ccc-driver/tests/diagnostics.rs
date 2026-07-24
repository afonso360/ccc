use std::fs;
use std::path::{Path, PathBuf};

mod support;

struct Case {
    name: &'static str,
    expected: &'static str,
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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
        let directory = support::TestWorkspace::new("diagnostics", case.name).retain_on_failure();
        let output = directory.join(format!("{}.o", case.name));
        let input = format!("tests/diagnostics/cases/{}.c", case.name);
        let result = support::ccc_command()
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

        directory.assert_command_failure(case.name, &result);
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
        let directory = support::TestWorkspace::new("diagnostics", case.name).retain_on_failure();
        let output = directory.join(format!("{}.o", case.name));
        let input = format!("tests/diagnostics/cases/{}.c", case.name);
        let result = support::ccc_command()
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

        directory.assert_command_failure(case.name, &result);
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
        let directory = support::TestWorkspace::new("diagnostics", name).retain_on_failure();
        let source = directory.join(format!("{name}.c"));
        let object = directory.join(format!("{name}.o"));
        fs::write(&source, text).unwrap();
        let result = support::ccc_command()
            .arg("-c")
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        directory.assert_command_failure(name, &result);
        assert!(stderr.contains("CCC2454"), "{name}: {stderr}");
        assert!(!object.exists(), "{name} emitted an object after rejection");
    }
}

#[test]
fn parser_recovery_reports_independent_errors_and_preserves_publications() {
    let repository = repository();
    let directory =
        support::TestWorkspace::new("diagnostics", "parser-recovery").retain_on_failure();
    let object = directory.join("recovered.o");
    let dependencies = directory.join("recovered.d");
    fs::write(&object, b"existing object").unwrap();
    fs::write(&dependencies, b"existing dependencies").unwrap();

    let result = support::ccc_command()
        .current_dir(&repository)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(["-nostdinc", "-ferror-limit=0", "-MD", "-MF"])
        .arg(&dependencies)
        .args(["-c", "tests/diagnostics/cases/parser-recovery.c", "-o"])
        .arg(&object)
        .output()
        .unwrap();

    directory.assert_command_failure("parser recovery", &result);
    assert!(result.stdout.is_empty());
    assert_eq!(
        String::from_utf8(result.stderr).unwrap(),
        include_str!("../../../tests/diagnostics/goldens/parser-recovery.stderr")
    );
    assert_eq!(fs::read(&object).unwrap(), b"existing object");
    assert_eq!(fs::read(&dependencies).unwrap(), b"existing dependencies");
}

#[test]
fn error_limit_is_shared_across_preprocessing_parsing_and_semantics() {
    let repository = repository();
    let run = |limit: usize| {
        support::ccc_command()
            .current_dir(&repository)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args([
                "-nostdinc",
                "-fsyntax-only",
                &format!("-ferror-limit={limit}"),
                "tests/diagnostics/cases/cross-stage-limit.c",
            ])
            .output()
            .unwrap()
    };

    let limited = run(3);
    let limited_stderr = String::from_utf8(limited.stderr).unwrap();
    assert!(!limited.status.success());
    assert_eq!(limited_stderr.matches("error[CCC1314]").count(), 1);
    assert_eq!(limited_stderr.matches("error[CCC1020]").count(), 2);
    assert_eq!(limited_stderr.matches("error[CCC0000]").count(), 1);
    assert!(!limited_stderr.contains("CCC2274"), "{limited_stderr}");

    let unlimited = run(0);
    let unlimited_stderr = String::from_utf8(unlimited.stderr).unwrap();
    assert!(!unlimited.status.success());
    assert_eq!(unlimited_stderr.matches("error[CCC1314]").count(), 1);
    assert_eq!(unlimited_stderr.matches("error[CCC1020]").count(), 2);
    assert_eq!(unlimited_stderr.matches("error[CCC2274]").count(), 1);
    assert!(!unlimited_stderr.contains("CCC0000"), "{unlimited_stderr}");
}

#[test]
fn json_diagnostics_match_the_versioned_deterministic_golden() {
    let repository = repository();
    let run = || {
        support::ccc_command()
            .current_dir(&repository)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args([
                "-nostdinc",
                "-fsyntax-only",
                "-ferror-limit=0",
                "-fdiagnostics-format=json",
                "tests/diagnostics/cases/parser-recovery.c",
            ])
            .output()
            .unwrap()
    };

    let first = run();
    let second = run();
    assert!(!first.status.success());
    assert!(first.stdout.is_empty());
    assert_eq!(first.stderr, second.stderr);
    assert_eq!(
        String::from_utf8(first.stderr).unwrap(),
        include_str!("../../../tests/diagnostics/goldens/parser-recovery.json")
    );
}

#[test]
fn json_diagnostics_carry_include_and_macro_provenance() {
    let repository = repository();
    let result = support::ccc_command()
        .current_dir(&repository)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args([
            "-nostdinc",
            "-fsyntax-only",
            "-ferror-limit=0",
            "-fdiagnostics-format=json",
            "-Itests/diagnostics/cases",
            "tests/diagnostics/cases/json-trace.c",
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.starts_with("{\"schema_version\":1,\"diagnostics\":["));
    assert!(stderr.contains("\"code\":\"CCC1314\""), "{stderr}");
    assert!(stderr.contains("\"code\":\"CCC1020\""), "{stderr}");
    assert!(stderr.contains("\"include_trace\":{\"truncated\":false,\"frames\":[{"));
    assert!(stderr.contains("\"kind\":\"macro_expansion\""), "{stderr}");
    assert!(stderr.contains("\"name\":\"BROKEN_TOKEN\""), "{stderr}");
    assert!(
        stderr.contains("tests/diagnostics/cases/json-trace.h"),
        "{stderr}"
    );
}

#[test]
fn json_mode_formats_command_line_parse_errors() {
    let result = support::ccc_command()
        .args(["-fdiagnostics-format=json", "--unsupported-for-json-test"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert!(!result.status.success());
    assert!(stderr.starts_with("{\"schema_version\":1,\"diagnostics\":["));
    assert_eq!(stderr.matches("\"schema_version\":1").count(), 1);
    assert!(stderr.contains("\"code\":\"CCC6000\""), "{stderr}");
    assert!(stderr.contains("\"category\":\"driver\""), "{stderr}");
    assert!(stderr.contains("unsupported option"), "{stderr}");
    assert!(stderr.ends_with("]}\n"), "{stderr}");
    assert!(!stderr.contains("\nccc:"), "{stderr}");
}

#[test]
fn recovery_poison_suppresses_only_dependent_semantic_errors() {
    let directory =
        support::TestWorkspace::new("diagnostics", "recovery-poison").retain_on_failure();
    let source = directory.join("recovery-poison.c");
    fs::write(
        &source,
        "int first(void) { int poisoned = ; return poisoned + independent; }\n\
         int second(void) { { int scoped = ; } return scoped; }\n",
    )
    .unwrap();

    let result = support::ccc_command()
        .args(["-nostdinc", "-fsyntax-only", "-ferror-limit=0"])
        .arg(&source)
        .output()
        .unwrap();
    directory.assert_command_failure("recovery poison", &result);
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert_eq!(stderr.matches("error[CCC1020]").count(), 2, "{stderr}");
    assert!(
        stderr.contains("undeclared identifier `independent`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("undeclared identifier `scoped`"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("undeclared identifier `poisoned`"),
        "{stderr}"
    );
}

#[test]
fn json_mode_composes_driver_and_frontend_diagnostics_once() {
    let repository = repository();
    let result = support::ccc_command()
        .current_dir(&repository)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args([
            "-nostdinc",
            "-E",
            "-fdiagnostics-format=json",
            "-fstack-protector-strong",
            "tests/preprocessing/diagnostics/warning.c",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert!(result.status.success(), "{stderr}");
    assert_eq!(
        stderr.matches("\"schema_version\":1").count(),
        1,
        "{stderr}"
    );
    assert!(stderr.starts_with("{\"schema_version\":1,\"diagnostics\":["));
    assert!(stderr.ends_with("]}\n"), "{stderr}");
    assert!(stderr.contains("\"code\":\"CCC6009\""), "{stderr}");
    assert!(stderr.contains("\"code\":\"CCC1315\""), "{stderr}");
    assert!(!stderr.contains("\nccc:"), "{stderr}");
}

#[test]
fn json_mode_composes_publication_failures_with_prior_warnings() {
    let repository = repository();
    let directory =
        support::TestWorkspace::new("diagnostics", "json-publication").retain_on_failure();
    let missing = directory.join("missing").join("output.d");
    let result = support::ccc_command()
        .current_dir(&repository)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(["-nostdinc", "-E", "-fdiagnostics-format=json", "-MD", "-MF"])
        .arg(&missing)
        .arg("tests/preprocessing/diagnostics/warning.c")
        .output()
        .unwrap();
    directory.assert_command_failure("JSON publication", &result);
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert_eq!(
        stderr.matches("\"schema_version\":1").count(),
        1,
        "{stderr}"
    );
    assert!(stderr.ends_with("]}\n"), "{stderr}");
    assert!(stderr.contains("\"code\":\"CCC1315\""), "{stderr}");
    assert!(stderr.contains("\"code\":\"CCC6000\""), "{stderr}");
    assert!(!stderr.contains("\nccc:"), "{stderr}");
}

#[test]
fn json_mode_composes_diagnostics_across_multiple_inputs() {
    let directory =
        support::TestWorkspace::new("diagnostics", "json-multiple-inputs").retain_on_failure();
    fs::write(
        directory.join("first.c"),
        "#warning first input\nint first;\n",
    )
    .unwrap();
    fs::write(
        directory.join("second.c"),
        "#warning second input\nint second = ;\n",
    )
    .unwrap();

    let result = support::ccc_command()
        .current_dir(directory.path())
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args([
            "-nostdinc",
            "-c",
            "-fdiagnostics-format=json",
            "first.c",
            "second.c",
        ])
        .output()
        .unwrap();
    directory.assert_command_failure("multiple JSON diagnostic inputs", &result);
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert_eq!(
        stderr.matches("\"schema_version\":1").count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        stderr.matches("\"code\":\"CCC1315\"").count(),
        2,
        "{stderr}"
    );
    assert!(stderr.contains("\"code\":\"CCC1020\""), "{stderr}");
    assert!(stderr.ends_with("]}\n"), "{stderr}");
    assert!(!stderr.contains("\nccc:"), "{stderr}");
}

#[test]
fn preprocessing_stops_when_the_shared_error_budget_is_exhausted() {
    let directory =
        support::TestWorkspace::new("diagnostics", "preprocessor-error-budget").retain_on_failure();
    let source = directory.join("budget.c");
    fs::write(
        &source,
        "#error stop here\n#include \"must-not-be-opened.h\"\nint malformed = ;\n",
    )
    .unwrap();
    let result = support::ccc_command()
        .args(["-nostdinc", "-fsyntax-only", "-ferror-limit=1"])
        .arg(&source)
        .output()
        .unwrap();
    directory.assert_command_failure("preprocessor error budget", &result);
    let stderr = String::from_utf8(result.stderr).unwrap();

    assert_eq!(stderr.matches("error[CCC1314]").count(), 1, "{stderr}");
    assert_eq!(stderr.matches("error[CCC0000]").count(), 1, "{stderr}");
    assert!(!stderr.contains("must-not-be-opened"), "{stderr}");
    assert!(!stderr.contains("CCC1020"), "{stderr}");
}

#[test]
fn failed_assembly_preserves_an_existing_object() {
    let directory =
        support::TestWorkspace::new("diagnostics", "assembly-publication").retain_on_failure();
    let source = directory.join("invalid.s");
    let object = directory.join("invalid.o");
    fs::write(&source, ".definitely_not_a_real_directive\n").unwrap();
    fs::write(&object, b"existing object").unwrap();

    let result = support::ccc_command()
        .args(["-c", "-x", "assembler"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();

    directory.assert_command_failure("assembly publication", &result);
    assert_eq!(fs::read(&object).unwrap(), b"existing object");
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
        let directory = support::TestWorkspace::new("diagnostics", name).retain_on_failure();
        let input = directory.join(format!("float16-{name}.c"));
        let output = directory.join(format!("float16-{name}.o"));
        fs::write(&input, source).unwrap();
        let result = support::ccc_command()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .args(["-nostdinc", "-c"])
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();

        directory.assert_command_success(name, &result);
        assert!(result.stdout.is_empty(), "{name} wrote stdout");
        assert!(result.stderr.is_empty(), "{name} wrote stderr");
        assert!(output.exists(), "{name} did not emit an object");
    }
}
