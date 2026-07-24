use std::fs;
use std::path::{Path, PathBuf};

mod support;

const PREPROCESSING: &[&str] = &["preprocessing", "pipeline"];
const SYNTAX: &[&str] = &["preprocessing", "parsing", "semantic_analysis", "pipeline"];
const IR: &[&str] = &[
    "preprocessing",
    "parsing",
    "semantic_analysis",
    "ccc_ir_lowering",
    "ccc_ir_optimization",
    "pipeline",
];
const CODEGEN: &[&str] = &[
    "preprocessing",
    "parsing",
    "semantic_analysis",
    "ccc_ir_lowering",
    "ccc_ir_optimization",
    "codegen.total",
    "pipeline",
];
const COMPILE: &[&str] = &[
    "preprocessing",
    "parsing",
    "semantic_analysis",
    "ccc_ir_lowering",
    "ccc_ir_optimization",
    "codegen.total",
    "object_packaging",
    "pipeline",
];

fn assert_phase_report(path: &Path, expected: &[&str]) {
    let report = fs::read_to_string(path).unwrap();
    let mut rows = report.lines();
    assert_eq!(rows.next(), Some("schema_version\t1"), "{report}");
    let parsed = rows
        .map(|row| {
            let (phase, value) = row
                .split_once('\t')
                .unwrap_or_else(|| panic!("phase-timing row is not TSV: {row:?}"));
            assert!(!value.contains('\t'), "extra TSV column in {row:?}");
            let value = value
                .parse::<u128>()
                .unwrap_or_else(|_| panic!("phase-timing value is not numeric: {row:?}"));
            (phase.to_owned(), value)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parsed
            .iter()
            .map(|(phase, _)| phase.as_str())
            .collect::<Vec<_>>(),
        expected,
        "{report}"
    );
    let pipeline = parsed
        .last()
        .filter(|(phase, _)| phase == "pipeline")
        .map(|(_, value)| *value)
        .expect("every successful report ends with pipeline");
    for (phase, value) in parsed.iter().take(parsed.len().saturating_sub(1)) {
        assert!(
            *value <= pipeline,
            "{phase} duration {value} exceeds pipeline duration {pipeline}"
        );
    }
}

fn run_timed_action(
    workspace: &support::TestWorkspace,
    source: &Path,
    name: &str,
    action: &[&str],
) -> PathBuf {
    let report = workspace.join(format!("{name}.tsv"));
    let result = support::ccc_command()
        .arg("-nostdinc")
        .args(action)
        .arg(source)
        .arg(format!("--write-phase-timings={}", report.display()))
        .output()
        .unwrap();
    workspace.assert_command_success(name, &result);
    report
}

#[test]
fn compile_reports_every_initial_phase_for_c_and_preprocessed_c() {
    let workspace = support::TestWorkspace::new("phase-timings", "compile").retain_on_failure();
    let source = workspace.write(
        "source.c",
        "int add(int left, int right) { return left + right; }\n",
    );
    let preprocessed = workspace.write(
        "source.i",
        "int add(int left, int right) { return left + right; }\n",
    );

    for (name, input) in [("c", source), ("preprocessed-c", preprocessed)] {
        let object = workspace.join(format!("{name}.o"));
        let report = workspace.join(format!("{name}.tsv"));
        let result = support::ccc_command()
            .arg("-nostdinc")
            .arg("-c")
            .arg(&input)
            .arg("-o")
            .arg(&object)
            .arg(format!("--write-phase-timings={}", report.display()))
            .output()
            .unwrap();
        workspace.assert_command_success(name, &result);
        assert!(object.is_file());
        assert_phase_report(&report, COMPILE);
    }
}

#[test]
fn syntax_preprocess_and_dump_reports_stop_at_the_last_executed_phase() {
    let workspace =
        support::TestWorkspace::new("phase-timings", "dump-truncation").retain_on_failure();
    let source = workspace.write(
        "source.c",
        "int add(int left, int right) { return left + right; }\n",
    );
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("preprocess", &["-E"], PREPROCESSING),
        ("pp-tokens", &["--dump-pp-tokens"], PREPROCESSING),
        ("tokens", &["--dump-tokens"], PREPROCESSING),
        (
            "ast",
            &["--dump-ast"],
            &["preprocessing", "parsing", "pipeline"],
        ),
        ("syntax", &["-fsyntax-only"], SYNTAX),
        ("typed-ast", &["--dump-typed-ast"], SYNTAX),
        ("ir", &["--dump-ir"], IR),
        ("abi", &["--dump-abi"], IR),
        ("clif", &["--emit=clif"], CODEGEN),
        ("codegen-stats", &["--emit=codegen-stats"], CODEGEN),
    ];

    for (name, action, expected) in cases {
        let report = run_timed_action(&workspace, &source, name, action);
        assert_phase_report(&report, expected);
    }
}

#[test]
fn rejected_driver_modes_never_create_or_replace_a_report() {
    let workspace =
        support::TestWorkspace::new("phase-timings", "rejected-modes").retain_on_failure();
    let source = workspace.write("source.c", "int value;\n");
    let second = workspace.write("second.c", "int other;\n");
    let assembly = workspace.write("startup.s", ".text\n");
    let source_text = source.display().to_string();
    let second_text = second.display().to_string();
    let assembly_text = assembly.display().to_string();
    let cases = [
        ("link", vec![source_text.as_str()]),
        ("commands", vec!["-###", "-c", source_text.as_str()]),
        ("verbose-version", vec!["-v"]),
        ("query", vec!["-dumpmachine"]),
        (
            "multiple",
            vec!["-c", source_text.as_str(), second_text.as_str()],
        ),
        ("assembly", vec!["-c", assembly_text.as_str()]),
        ("dependencies", vec!["-M", source_text.as_str()]),
    ];

    for (name, arguments) in cases {
        let report = workspace.join(format!("{name}.tsv"));
        fs::write(&report, "old report\n").unwrap();
        let result = support::ccc_command()
            .arg(format!("--write-phase-timings={}", report.display()))
            .args(arguments)
            .output()
            .unwrap();
        workspace.assert_command_failure(name, &result);
        assert_eq!(
            fs::read_to_string(&report).unwrap(),
            "old report\n",
            "{name}"
        );
    }
}

#[test]
fn help_and_version_override_timing_without_touching_the_sidecar() {
    let workspace =
        support::TestWorkspace::new("phase-timings", "early-actions").retain_on_failure();
    for (name, action) in [("help", "--help"), ("version", "--version")] {
        let report = workspace.join(format!("{name}.tsv"));
        fs::write(&report, "old report\n").unwrap();
        for arguments in [
            vec![
                format!("--write-phase-timings={}", report.display()),
                action.to_owned(),
            ],
            vec![
                action.to_owned(),
                format!("--write-phase-timings={}", report.display()),
            ],
        ] {
            let result = support::ccc_command().args(arguments).output().unwrap();
            workspace.assert_command_success(name, &result);
            assert_eq!(fs::read_to_string(&report).unwrap(), "old report\n");
        }
    }
}

#[test]
fn failed_translations_and_output_publications_preserve_an_old_report() {
    let workspace = support::TestWorkspace::new("phase-timings", "failures").retain_on_failure();
    let preprocess_error = workspace.write("preprocess-error.c", "#include \"does-not-exist.h\"\n");
    let parse_error = workspace.write("parse-error.c", "int broken(\n");
    let semantic_error = workspace.write(
        "semantic-error.c",
        "int value(void) { return undeclared_name; }\n",
    );
    let valid = workspace.write("valid.c", "int value(void) { return 0; }\n");
    let output_directory = workspace.join("object-output");
    fs::create_dir(&output_directory).unwrap();
    fs::write(output_directory.join("marker"), "old object destination").unwrap();
    let dependency_directory = workspace.join("dependency-output");
    fs::create_dir(&dependency_directory).unwrap();
    fs::write(
        dependency_directory.join("marker"),
        "old dependency destination",
    )
    .unwrap();
    let object = workspace.join("dependency-failure.o");

    let cases = [
        (
            "preprocessing",
            vec![
                "--dump-ast".to_owned(),
                preprocess_error.display().to_string(),
            ],
        ),
        (
            "parsing",
            vec![
                "-fsyntax-only".to_owned(),
                parse_error.display().to_string(),
            ],
        ),
        (
            "semantics",
            vec![
                "-fsyntax-only".to_owned(),
                semantic_error.display().to_string(),
            ],
        ),
        (
            "object-publication",
            vec![
                "-c".to_owned(),
                valid.display().to_string(),
                "-o".to_owned(),
                output_directory.display().to_string(),
            ],
        ),
        (
            "dependency-publication",
            vec![
                "-c".to_owned(),
                "-MD".to_owned(),
                "-MF".to_owned(),
                dependency_directory.display().to_string(),
                valid.display().to_string(),
                "-o".to_owned(),
                object.display().to_string(),
            ],
        ),
    ];

    for (name, arguments) in cases {
        let report = workspace.join(format!("{name}.tsv"));
        fs::write(&report, "old report\n").unwrap();
        let result = support::ccc_command()
            .arg("-nostdinc")
            .arg(format!("--write-phase-timings={}", report.display()))
            .args(arguments)
            .output()
            .unwrap();
        workspace.assert_command_failure(name, &result);
        assert_eq!(
            fs::read_to_string(&report).unwrap(),
            "old report\n",
            "{name}"
        );
    }
}

#[test]
fn sidecar_publication_failure_is_atomic_and_happens_after_compilation() {
    let workspace = support::TestWorkspace::new("phase-timings", "publication").retain_on_failure();
    let source = workspace.write("source.c", "int value(void) { return 0; }\n");
    let object = workspace.join("source.o");
    let report = workspace.join("report.tsv");
    fs::create_dir(&report).unwrap();
    fs::write(report.join("marker"), "old report destination").unwrap();

    let result = support::ccc_command()
        .arg("-nostdinc")
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .arg(format!("--write-phase-timings={}", report.display()))
        .output()
        .unwrap();
    workspace.assert_command_failure("publish phase timings", &result);

    assert!(
        object.is_file(),
        "the compilation completes before timing publication"
    );
    assert_eq!(
        fs::read_to_string(report.join("marker")).unwrap(),
        "old report destination"
    );
    assert_eq!(fs::read_dir(&report).unwrap().count(), 1);
}

#[test]
fn timing_output_cannot_replace_forced_or_discovered_preprocessing_inputs() {
    let workspace =
        support::TestWorkspace::new("phase-timings", "source-protection").retain_on_failure();
    let header = workspace.write("values.h", "#define VALUE 7\n");
    let source = workspace.write(
        "source.c",
        "#include \"values.h\"\nint value(void) { return VALUE; }\n",
    );

    let discovered = support::ccc_command()
        .arg("-nostdinc")
        .arg("--dump-ast")
        .arg(&source)
        .arg(format!("--write-phase-timings={}", header.display()))
        .output()
        .unwrap();
    workspace.assert_command_failure("discovered source path protection", &discovered);
    assert_eq!(fs::read_to_string(&header).unwrap(), "#define VALUE 7\n");

    let forced = support::ccc_command()
        .arg("-nostdinc")
        .arg("-fsyntax-only")
        .arg("-include")
        .arg(&header)
        .arg(&source)
        .arg(format!("--write-phase-timings={}", header.display()))
        .output()
        .unwrap();
    workspace.assert_command_failure("forced source path protection", &forced);
    assert_eq!(fs::read_to_string(&header).unwrap(), "#define VALUE 7\n");
}

#[cfg(target_os = "macos")]
#[test]
fn case_folded_explicit_and_implicit_object_aliases_cannot_be_overwritten() {
    let workspace =
        support::TestWorkspace::new("phase-timings", "case-aliases").retain_on_failure();
    let probe = workspace.write("case-probe", "probe");
    if !workspace.join("CASE-PROBE").exists() {
        // APFS can be configured case-sensitively. Different-case names are
        // distinct outputs on such a volume and are not aliases to reject.
        return;
    }
    assert!(probe.is_file());

    let explicit_source = workspace.write("explicit.c", "int value(void) { return 0; }\n");
    let explicit_object = workspace.join("Artifact.o");
    let explicit_report = workspace.join("artifact.o");
    let explicit = support::ccc_command()
        .arg("-nostdinc")
        .arg("-c")
        .arg(&explicit_source)
        .arg("-o")
        .arg(&explicit_object)
        .arg(format!(
            "--write-phase-timings={}",
            explicit_report.display()
        ))
        .output()
        .unwrap();
    workspace.assert_command_failure("explicit case-folded output alias", &explicit);
    object::File::parse(fs::read(&explicit_object).unwrap().as_slice())
        .expect("the explicit object must not be replaced by timing TSV");

    let _implicit_source = workspace.write("Implicit.c", "int other(void) { return 1; }\n");
    let implicit_object = workspace.join("Implicit.o");
    let implicit_report = workspace.join("implicit.o");
    let implicit = support::ccc_command()
        .current_dir(workspace.path())
        .arg("-nostdinc")
        .arg("-c")
        .arg("Implicit.c")
        .arg(format!(
            "--write-phase-timings={}",
            implicit_report.display()
        ))
        .output()
        .unwrap();
    workspace.assert_command_failure("implicit case-folded output alias", &implicit);
    object::File::parse(fs::read(&implicit_object).unwrap().as_slice())
        .expect("the implicit object must not be replaced by timing TSV");

    let normalization_probe = workspace.write("normalize-\u{e9}", "probe");
    if workspace.join("normalize-e\u{301}").exists() {
        assert!(normalization_probe.is_file());
        let unicode_source =
            workspace.write("unicode.c", "int unicode_value(void) { return 2; }\n");
        let unicode_object = workspace.join("Caf\u{e9}.o");
        let unicode_report = workspace.join("Cafe\u{301}.o");
        let unicode = support::ccc_command()
            .arg("-nostdinc")
            .arg("-c")
            .arg(&unicode_source)
            .arg("-o")
            .arg(&unicode_object)
            .arg(format!(
                "--write-phase-timings={}",
                unicode_report.display()
            ))
            .output()
            .unwrap();
        workspace.assert_command_failure("Unicode-normalized output alias", &unicode);
        object::File::parse(fs::read(&unicode_object).unwrap().as_slice())
            .expect("the Unicode-named object must not be replaced by timing TSV");
    }
}
