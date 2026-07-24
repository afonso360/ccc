use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use ccc_target::ENABLED_TARGET_SPECS;
use object::{Object as _, ObjectSymbol as _};

mod support;

trait PreprocessingWorkspaceExt {
    fn command(&self) -> Command;
    fn command_for_target(&self, target: &str) -> Command;
    fn host_command(&self) -> Command;
}

impl PreprocessingWorkspaceExt for support::TestWorkspace {
    fn command(&self) -> Command {
        self.command_for_target("x86_64-unknown-linux-gnu")
    }

    fn command_for_target(&self, target: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
        command
            .current_dir(self.path())
            .arg(format!("--target={target}"))
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env_remove("SOURCE_DATE_EPOCH");
        command
    }

    fn host_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
        command
            .current_dir(self.path())
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env_remove("SOURCE_DATE_EPOCH");
        command
    }
}

struct RunResult {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    workspace: Option<PathBuf>,
}

impl RunResult {
    #[track_caller]
    fn assert_success(&self) {
        support::assert_command_text_success(
            "CCC invocation",
            &self.status,
            &self.stdout,
            &self.stderr,
            self.workspace.as_deref(),
        );
    }

    #[track_caller]
    fn assert_failure(&self) {
        support::assert_command_text_failure(
            "CCC invocation",
            &self.status,
            &self.stdout,
            &self.stderr,
            self.workspace.as_deref(),
        );
    }
}

fn run(mut command: Command) -> RunResult {
    let workspace = command.get_current_dir().map(Path::to_path_buf);
    let output = command.output().unwrap();
    RunResult {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
        workspace,
    }
}

fn run_reference_preprocessor(
    directory: &support::TestWorkspace,
    source: &Path,
) -> Option<RunResult> {
    let output = Command::new("cc")
        .current_dir(directory.path())
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("SOURCE_DATE_EPOCH")
        .args(["-E", "-P", "-nostdinc"])
        .arg(source)
        .output()
        .ok()?;
    Some(RunResult {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        workspace: Some(directory.path().to_owned()),
    })
}

fn squash_whitespace(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn make_quote_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('$', "$$")
        .replace('#', "\\#")
        .replace(' ', "\\ ")
}

fn repository_fixture(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn macos_sdk_root() -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("the Darwin hosted-header gate requires xcrun");
    support::assert_command_success("locate the macOS SDK with xcrun", &output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn apple_math_private_builtin_adaptation_is_scoped_and_predicate_neutral() {
    let directory = support::TestWorkspace::new("preprocessing", "apple-math-private-builtins")
        .retain_on_failure();
    let source = directory.write(
        "apple-math.c",
        concat!(
            "#if __has_builtin(__builtin_fabsf)\n",
            "#define CCC_FABSF_BEFORE 1\n",
            "#else\n",
            "#define CCC_FABSF_BEFORE 0\n",
            "#endif\n",
            "#if __has_builtin(__builtin_fabs)\n",
            "#define CCC_FABS_BEFORE 1\n",
            "#else\n",
            "#define CCC_FABS_BEFORE 0\n",
            "#endif\n",
            "#if __has_builtin(__builtin_fabsl)\n",
            "#define CCC_FABSL_BEFORE 1\n",
            "#else\n",
            "#define CCC_FABSL_BEFORE 0\n",
            "#endif\n",
            "#if __has_builtin(__builtin_inf)\n",
            "#define CCC_INF_BEFORE 1\n",
            "#else\n",
            "#define CCC_INF_BEFORE 0\n",
            "#endif\n",
            "#include <math.h>\n",
            "#ifdef __builtin_fabsf\n",
            "#error private fabsf spelling leaked from math.h\n",
            "#endif\n",
            "#ifdef __builtin_fabs\n",
            "#error private fabs spelling leaked from math.h\n",
            "#endif\n",
            "#ifdef __builtin_fabsl\n",
            "#error private fabsl spelling leaked from math.h\n",
            "#endif\n",
            "#ifdef __builtin_inf\n",
            "#error private inf spelling leaked from math.h\n",
            "#endif\n",
            "#ifdef __builtin_infl\n",
            "#error private infl spelling leaked from math.h\n",
            "#endif\n",
            "#if CCC_FABSF_BEFORE != __has_builtin(__builtin_fabsf)\n",
            "#error math.h changed __has_builtin(fabsf)\n",
            "#endif\n",
            "#if CCC_FABS_BEFORE != __has_builtin(__builtin_fabs)\n",
            "#error math.h changed __has_builtin(fabs)\n",
            "#endif\n",
            "#if CCC_FABSL_BEFORE != __has_builtin(__builtin_fabsl)\n",
            "#error math.h changed __has_builtin(fabsl)\n",
            "#endif\n",
            "#if CCC_INF_BEFORE != __has_builtin(__builtin_inf)\n",
            "#error math.h changed __has_builtin(inf)\n",
            "#endif\n",
            "long double (*ccc_public_fabsl)(long double) = fabsl;\n",
            "int ccc_public_isfinite(double value) { return isfinite(value); }\n",
        ),
    );

    let sdk = macos_sdk_root();
    let mut command = directory.host_command();
    command
        .args(["--target=aarch64-apple-darwin", "--sdk-root"])
        .arg(sdk)
        .args(["-E", "-P"])
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("int__inline_isfinitef(float"), "{output}");
    assert!(output.contains("__builtin_huge_val()"), "{output}");
    assert!(!output.contains("__builtin_fabs"), "{output}");
    assert!(!output.contains("__builtin_inf()"), "{output}");
    assert!(!output.contains("__builtin_infl()"), "{output}");
    assert!(
        output.contains("longdouble(*ccc_public_fabsl)(longdouble)=fabsl;"),
        "{output}"
    );
}

fn normalize_fixture_snapshot(text: &str) -> String {
    let mut normalized = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

#[test]
fn committed_preprocessing_fixtures_match_their_goldens() {
    let cases = [
        (
            "tests/preprocessing/macros/basic.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/macros-basic.out"),
        ),
        (
            "tests/preprocessing/macros/advanced.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/macros-advanced.out"),
        ),
        (
            "tests/preprocessing/conditionals/expression.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/conditionals-expression.out"),
        ),
        (
            "tests/preprocessing/conditionals/inactive.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/conditionals-inactive.out"),
        ),
        (
            "tests/preprocessing/includes/main.c",
            Some("tests/preprocessing/includes"),
            include_str!("../../../tests/preprocessing/goldens/includes-main.out"),
        ),
        (
            "tests/preprocessing/includes/computed.c",
            Some("tests/preprocessing/includes"),
            include_str!("../../../tests/preprocessing/goldens/includes-computed.out"),
        ),
        (
            "tests/preprocessing/predefined/main.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/predefined.out"),
        ),
        (
            "tests/preprocessing/line/main.c",
            None,
            include_str!("../../../tests/preprocessing/goldens/line.out"),
        ),
        (
            "tests/preprocessing/pragmas/main.c",
            Some("tests/preprocessing/pragmas"),
            include_str!("../../../tests/preprocessing/goldens/pragmas.out"),
        ),
    ];
    let repository = repository_fixture("");

    for (source, include, expected) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
        command
            .current_dir(&repository)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("SOURCE_DATE_EPOCH", "0")
            .args(["-E", "-P", "-nostdinc"]);
        if let Some(include) = include {
            command.arg("-I").arg(repository.join(include));
        }
        command.arg(repository.join(source));
        let result = run(command);
        result.assert_success();
        assert!(result.stderr.is_empty(), "{source}: {}", result.stderr);
        assert_eq!(
            normalize_fixture_snapshot(&result.stdout),
            expected,
            "{source}"
        );
    }
}

#[test]
fn committed_warning_fixture_matches_its_diagnostic_golden() {
    let repository = repository_fixture("");
    let source = repository.join("tests/preprocessing/diagnostics/warning.c");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
    command
        .current_dir(&repository)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["-E", "-P", "-nostdinc"])
        .arg(source);
    let result = run(command);
    result.assert_success();
    let repository_prefix = repository.to_string_lossy();
    let repository_prefix = repository_prefix.trim_end_matches(std::path::MAIN_SEPARATOR);
    assert_eq!(
        normalize_fixture_snapshot(&result.stdout),
        include_str!("../../../tests/preprocessing/goldens/diagnostics-warning.out")
    );
    assert_eq!(
        result.stderr.replace(repository_prefix, "<repo>"),
        include_str!("../../../tests/preprocessing/goldens/diagnostics-warning.stderr")
    );
}

#[test]
fn preprocess_output_tracks_file_entry_return_and_marker_suppression() {
    let directory =
        support::TestWorkspace::new("preprocessing", "line-markers").retain_on_failure();
    let source = directory.write(
        "source/main.c",
        "#include \"value.h\"\nint from_main = FROM_HEADER;\n",
    );
    directory.write(
        "source/value.h",
        "#define FROM_HEADER 7\nint from_header = FROM_HEADER;\n",
    );

    let mut command = directory.command();
    command.args(["-E", "-nostdinc"]).arg(&source);
    let marked = run(command);
    marked.assert_success();
    assert!(
        marked
            .stdout
            .lines()
            .any(|line| line == format!("# 1 \"{}\"", source.display())),
        "main-file marker should not carry an entry flag:\n{}",
        marked.stdout
    );
    assert!(
        marked.stdout.lines().any(|line| {
            line.starts_with("# 1 ") && line.contains("\"value.h\"") && line.ends_with(" 1")
        }),
        "missing header-entry marker:\n{}",
        marked.stdout
    );
    assert!(
        marked.stdout.lines().any(|line| {
            line.starts_with("# 2 ")
                && line.contains(&format!("\"{}\"", source.display()))
                && line.ends_with(" 2")
        }),
        "missing include-return marker:\n{}",
        marked.stdout
    );
    let marked_text = squash_whitespace(&marked.stdout);
    assert!(marked_text.contains("intfrom_header=7;"));
    assert!(marked_text.contains("intfrom_main=7;"));

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(&source);
    let unmarked = run(command);
    unmarked.assert_success();
    assert!(
        unmarked
            .stdout
            .lines()
            .all(|line| !line.trim_start().starts_with("# ")),
        "-P left a linemarker behind:\n{}",
        unmarked.stdout
    );
    assert!(squash_whitespace(&unmarked.stdout).contains("intfrom_main=7;"));
}

#[test]
fn expands_object_function_stringize_paste_and_variadic_macros() {
    let directory =
        support::TestWorkspace::new("preprocessing", "macro-expansion").retain_on_failure();
    let source = directory.write(
        "macros.c",
        concat!(
            "#define OBJECT 7\n",
            "#define CAT_INNER(left, right) left ## right\n",
            "#define CAT(left, right) CAT_INNER(left, right)\n",
            "#define STRINGIZE_INNER(value) #value\n",
            "#define STRINGIZE(value) STRINGIZE_INNER(value)\n",
            "#define CALL(first, ...) sink(first, ## __VA_ARGS__)\n",
            "#define NAMED(first, rest...) sink(first, ## rest)\n",
            "int CAT(pasted, _name) = OBJECT;\n",
            "const char *text = STRINGIZE(alpha beta);\n",
            "int sum = ADD(VALUE, 2);\n",
            "void invoke(void) { CALL(1); CALL(1, 2); }\n",
            "void named(void) { NAMED(3); NAMED(3, 4); }\n",
        ),
    );

    let mut command = directory.command();
    command
        .args([
            "-E",
            "-P",
            "-nostdinc",
            "-DVALUE=39",
            "-UVALUE",
            "-DVALUE=40",
            "-DADD(left,right)=((left)+(right))",
        ])
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("intpasted_name=7;"), "{output}");
    assert!(output.contains("constchar*text=\"alphabeta\";"), "{output}");
    assert!(output.contains("intsum=((40)+(2));"), "{output}");
    assert!(
        output.contains("voidinvoke(void){sink(1);sink(1,2);}"),
        "{output}"
    );
    assert!(
        output.contains("voidnamed(void){sink(3);sink(3,4);}"),
        "{output}"
    );
}

#[test]
fn resolves_nested_computed_and_ordered_include_paths() {
    let directory =
        support::TestWorkspace::new("preprocessing", "include-search").retain_on_failure();
    let source = directory.write(
        "source/main.c",
        concat!(
            "#include \"local.h\"\n",
            "#include \"quote_only.h\"\n",
            "#include <pick.h>\n",
            "#define COMPUTED_HEADER <computed.h>\n",
            "#include COMPUTED_HEADER\n",
            "#include <tier.h>\n",
            "int include_sentinel = LOCAL + DEEP + QUOTED + PICKED + COMPUTED + TIER;\n",
        ),
    );
    directory.write(
        "source/local.h",
        "#define LOCAL 1\n#include \"detail/deep.h\"\n",
    );
    directory.write("source/detail/deep.h", "#define DEEP 2\n");
    let quote = directory.join("quote");
    directory.write("quote/quote_only.h", "#define QUOTED 3\n");
    let first_user = directory.join("user-first");
    directory.write("user-first/pick.h", "#define PICKED 4\n");
    directory.write("user-first/computed.h", "#define COMPUTED 5\n");
    let second_user = directory.join("user-second");
    directory.write("user-second/pick.h", "#define PICKED 40\n");
    let system = directory.join("system");
    directory.write("system/tier.h", "#define TIER 6\n");
    let after = directory.join("after");
    directory.write("after/tier.h", "#define TIER 60\n");

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-iquote"])
        .arg(quote)
        .arg("-I")
        .arg(first_user)
        .arg("-I")
        .arg(second_user)
        .arg("-isystem")
        .arg(system)
        .arg("-idirafter")
        .arg(after)
        .arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intinclude_sentinel=1+2+3+4+5+6;"),
        "{}",
        result.stdout
    );
}

#[test]
fn evaluates_conditional_expressions_and_feature_predicates() {
    let directory =
        support::TestWorkspace::new("preprocessing", "conditionals").retain_on_failure();
    let source = directory.write(
        "conditional.c",
        concat!(
            "#define FLAG 1\n",
            "#if defined FLAG && ((3 << 2) == 12) && ('A' == 65) \\\n",
            " && ((0 ? 1 : 7) == 7) && __has_include(\"present.h\") \\\n",
            " && !__has_include(\"missing.h\") && !__has_attribute(ccc_never)\n",
            "int conditional_sentinel = 42;\n",
            "#else\n",
            "#error conditional evaluator selected the wrong branch\n",
            "#endif\n",
            "#if 0\n",
            "#include \"must_not_be_opened.h\"\n",
            "#endif\n",
        ),
    );
    directory.write("present.h", "#define PRESENT 1\n");

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intconditional_sentinel=42;"),
        "{}",
        result.stdout
    );
}

#[test]
fn inactive_groups_tolerate_unterminated_literals() {
    let directory = support::TestWorkspace::new("preprocessing", "inactive-token-validation")
        .retain_on_failure();
    let source = directory.write(
        "invalid.c",
        "#if 0\nconst char *text = \"unterminated;\nthis isn't C @ all\n#endif\nint live_value;\n",
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intlive_value;"),
        "{}",
        result.stdout
    );
    assert!(result.stderr.trim().is_empty(), "{}", result.stderr);
}

#[test]
fn emits_predefined_dynamic_and_reproducible_macros() {
    let directory =
        support::TestWorkspace::new("preprocessing", "predefined-macros").retain_on_failure();
    let source = directory.write(
        "predefined.c",
        concat!(
            "int standard = __STDC__;\n",
            "long standard_version = __STDC_VERSION__;\n",
            "int compatibility = __GNUC__ * 100 + __GNUC_MINOR__;\n",
            "int pointer_size = __SIZEOF_POINTER__;\n",
            "const char *translation_date = __DATE__;\n",
            "const char *translation_time = __TIME__;\n",
            "const char *translation_file = __FILE__;\n",
            "int translation_line = __LINE__;\n",
            "int double_mantissa_bits = __DBL_MANT_DIG__;\n",
            "int float_mantissa_bits = __FLT_MANT_DIG__;\n",
            "int float16_mantissa_bits = __FLT16_MANT_DIG__;\n",
            "_Float16 float16_maximum = __FLT16_MAX__;\n",
            "_Float16 float16_epsilon = __FLT16_EPSILON__;\n",
            "float float_maximum = __FLT_MAX__;\n",
            "float float_epsilon = __FLT_EPSILON__;\n",
            "int double_decimal_digits = __DBL_DIG__;\n",
            "int double_max_decimal_exponent = __DBL_MAX_10_EXP__;\n",
            "double double_maximum = __DBL_MAX__;\n",
            "double double_epsilon = __DBL_EPSILON__;\n",
            "int counters[] = { __COUNTER__, __COUNTER__ };\n",
            "#define CCC_STRINGIFY_INNER(value) #value\n",
            "#define CCC_STRINGIFY(value) CCC_STRINGIFY_INNER(value)\n",
            "const char *user_label_prefix = CCC_STRINGIFY(__USER_LABEL_PREFIX__);\n",
            "__SIZE_TYPE__ size_value;\n",
            "__PTRDIFF_TYPE__ difference_value;\n",
            "__WCHAR_TYPE__ wide_value;\n",
            "__INTMAX_TYPE__ maximum_value;\n",
            "#if __SIZE_MAX__ != 18446744073709551615UL\n",
            "#error invalid size limit\n",
            "#endif\n",
        ),
    );

    let mut command = directory.command();
    command
        .env("SOURCE_DATE_EPOCH", "951827696")
        .args(["-E", "-P", "-nostdinc"])
        .arg(&source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("intstandard=1;"), "{output}");
    assert!(output.contains("longstandard_version=201112L;"), "{output}");
    assert!(output.contains("intcompatibility=4*100+2;"), "{output}");
    assert!(output.contains("intpointer_size=8;"), "{output}");
    assert!(output.contains("intdouble_mantissa_bits=53;"), "{output}");
    assert!(output.contains("intfloat_mantissa_bits=24;"), "{output}");
    assert!(output.contains("intfloat16_mantissa_bits=11;"), "{output}");
    assert!(
        output.contains("_Float16float16_maximum=0x1.ffcp+15;"),
        "{output}"
    );
    assert!(
        output.contains("_Float16float16_epsilon=0x1p-10;"),
        "{output}"
    );
    assert!(
        output.contains("floatfloat_maximum=0x1.fffffep+127F;"),
        "{output}"
    );
    assert!(output.contains("floatfloat_epsilon=0x1p-23F;"), "{output}");
    assert!(output.contains("intdouble_decimal_digits=15;"), "{output}");
    assert!(
        output.contains("intdouble_max_decimal_exponent=308;"),
        "{output}"
    );
    assert!(
        output.contains("doubledouble_maximum=0x1.fffffffffffffp+1023;"),
        "{output}"
    );
    assert!(output.contains("doubledouble_epsilon=0x1p-52;"), "{output}");
    assert!(
        output.contains("constchar*user_label_prefix=\"\";"),
        "{output}"
    );
    assert!(result.stdout.contains("\"Feb 29 2000\""));
    assert!(result.stdout.contains("\"12:34:56\""));
    assert!(result.stdout.contains(&format!("\"{}\"", source.display())));
    assert!(output.contains("inttranslation_line=8;"), "{output}");
    assert!(output.contains("intcounters[]={0,1};"), "{output}");
    assert!(output.contains("longunsignedintsize_value;"), "{output}");
    assert!(output.contains("longintdifference_value;"), "{output}");
    assert!(output.contains("intwide_value;"), "{output}");
    assert!(output.contains("longintmaximum_value;"), "{output}");

    let mut command = directory.command();
    command.args(["-dM", "-E", "-nostdinc"]).arg(source);
    let macros = run(command);
    macros.assert_success();
    for definition in [
        "#define __CCC__ 1",
        "#define __DBL_DECIMAL_DIG__ 17",
        "#define __DBL_DENORM_MIN__ 0x1p-1074",
        "#define __DBL_DIG__ 15",
        "#define __DBL_EPSILON__ 0x1p-52",
        "#define __DBL_HAS_DENORM__ 1",
        "#define __DBL_HAS_INFINITY__ 1",
        "#define __DBL_HAS_QUIET_NAN__ 1",
        "#define __DBL_MANT_DIG__ 53",
        "#define __DBL_MAX_10_EXP__ 308",
        "#define __DBL_MAX_EXP__ 1024",
        "#define __DBL_MAX__ 0x1.fffffffffffffp+1023",
        "#define __DBL_MIN_10_EXP__ (-307)",
        "#define __DBL_MIN_EXP__ (-1021)",
        "#define __DBL_MIN__ 0x1p-1022",
        "#define __DBL_NORM_MAX__ 0x1.fffffffffffffp+1023",
        "#define __FLT_EVAL_METHOD__ 0",
        "#define __FLT16_DECIMAL_DIG__ 5",
        "#define __FLT16_DENORM_MIN__ 0x1p-24",
        "#define __FLT16_DIG__ 3",
        "#define __FLT16_EPSILON__ 0x1p-10",
        "#define __FLT16_HAS_DENORM__ 1",
        "#define __FLT16_HAS_INFINITY__ 1",
        "#define __FLT16_HAS_QUIET_NAN__ 1",
        "#define __FLT16_MANT_DIG__ 11",
        "#define __FLT16_MAX_10_EXP__ 4",
        "#define __FLT16_MAX_EXP__ 16",
        "#define __FLT16_MAX__ 0x1.ffcp+15",
        "#define __FLT16_MIN_10_EXP__ (-4)",
        "#define __FLT16_MIN_EXP__ (-13)",
        "#define __FLT16_MIN__ 0x1p-14",
        "#define __FLT16_NORM_MAX__ 0x1.ffcp+15",
        "#define __FLT_DECIMAL_DIG__ 9",
        "#define __FLT_DENORM_MIN__ 0x1p-149F",
        "#define __FLT_DIG__ 6",
        "#define __FLT_EPSILON__ 0x1p-23F",
        "#define __FLT_HAS_DENORM__ 1",
        "#define __FLT_HAS_INFINITY__ 1",
        "#define __FLT_HAS_QUIET_NAN__ 1",
        "#define __FLT_MANT_DIG__ 24",
        "#define __FLT_MAX_10_EXP__ 38",
        "#define __FLT_MAX_EXP__ 128",
        "#define __FLT_MAX__ 0x1.fffffep+127F",
        "#define __FLT_MIN_10_EXP__ (-37)",
        "#define __FLT_MIN_EXP__ (-125)",
        "#define __FLT_MIN__ 0x1p-126F",
        "#define __FLT_NORM_MAX__ 0x1.fffffep+127F",
        "#define __FLT_RADIX__ 2",
        "#define __GNUC__ 4",
        "#define __INTMAX_TYPE__ long int",
        "#define __PTRDIFF_TYPE__ long int",
        "#define __SIZE_MAX__ 18446744073709551615UL",
        "#define __SIZEOF_FLOAT16__ 2",
        "#define __SIZEOF_POINTER__ 8",
        "#define __SIZE_TYPE__ long unsigned int",
        "#define __STDC__ 1",
        "#define __STDC_VERSION__ 201112L",
        "#define __USER_LABEL_PREFIX__",
        "#define __WCHAR_TYPE__ int",
    ] {
        assert!(
            macros.stdout.lines().any(|line| line == definition),
            "missing {definition:?} in:\n{}",
            macros.stdout
        );
    }
    let names = macros
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("#define "))
        .filter_map(|definition| definition.split_whitespace().next())
        .collect::<Vec<_>>();
    assert!(
        names.windows(2).all(|pair| pair[0] <= pair[1]),
        "macro dump is not sorted:\n{}",
        macros.stdout
    );
}

#[test]
fn optimization_profiles_control_the_predefined_macro_contract() {
    let directory =
        support::TestWorkspace::new("preprocessing", "optimization-macros").retain_on_failure();
    let source = directory.write("empty.c", "\n");

    for (optimization, optimize, size) in [
        (None, false, false),
        (Some("-O0"), false, false),
        (Some("-O2"), true, false),
        (Some("-Os"), true, true),
        (Some("-Oz"), true, true),
    ] {
        let mut command = directory.command();
        command.args(["-dM", "-E", "-nostdinc"]);
        if let Some(optimization) = optimization {
            command.arg(optimization);
        }
        command.arg(&source);
        let macros = run(command);
        macros.assert_success();
        assert_eq!(macros.stdout.contains("#define __OPTIMIZE__ 1\n"), optimize);
        assert_eq!(
            macros.stdout.contains("#define __OPTIMIZE_SIZE__ 1\n"),
            size
        );
        assert!(!macros.stdout.contains("#define __NO_INLINE__ 1\n"));
    }
}

#[test]
fn expands_computed_line_operands_and_logical_locations() {
    let directory =
        support::TestWorkspace::new("preprocessing", "line-directive").retain_on_failure();
    let source = directory.write(
        "line.c",
        concat!(
            "#define GENERATED_LINE 80\n",
            "#define GENERATED_FILE \"generated.c\"\n",
            "#line GENERATED_LINE GENERATED_FILE\n",
            "int generated_line = __LINE__;\n",
            "const char *generated_file = __FILE__;\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        result
            .stdout
            .lines()
            .any(|line| line.starts_with("# 80 \"generated.c\"")),
        "missing logical-location marker:\n{}",
        result.stdout
    );
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("intgenerated_line=80;"), "{output}");
    assert!(
        output.contains("constchar*generated_file=\"generated.c\";"),
        "{output}"
    );
}

#[test]
fn warning_controls_promote_and_suppress_preprocessor_warnings() {
    let directory =
        support::TestWorkspace::new("preprocessing", "warning-controls").retain_on_failure();
    let source = directory.write(
        "warning.c",
        "#warning this is an intentional warning\nint warning_sentinel;\n",
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(&source);
    let warning = run(command);
    warning.assert_success();
    assert!(warning.stderr.contains("warning"), "{}", warning.stderr);
    assert!(
        warning.stderr.contains("intentional warning"),
        "{}",
        warning.stderr
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-Werror"])
        .arg(&source);
    let promoted = run(command);
    promoted.assert_failure();
    assert!(promoted.stderr.contains("error"), "{}", promoted.stderr);

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc", "-w"]).arg(&source);
    let suppressed = run(command);
    suppressed.assert_success();
    assert!(
        !suppressed.stderr.contains("intentional warning"),
        "{}",
        suppressed.stderr
    );

    for arguments in [
        vec!["-Wno-cpp", "-Wcpp"],
        vec!["-Werror", "-Wno-error=cpp"],
        vec!["-Wno-error=cpp", "-Werror"],
    ] {
        let mut command = directory.command();
        command
            .args(["-E", "-P", "-nostdinc"])
            .args(&arguments)
            .arg(&source);
        let demoted = run(command);
        demoted.assert_success();
        assert!(
            demoted.stderr.contains("intentional warning"),
            "{arguments:?}: {}",
            demoted.stderr
        );
    }

    for arguments in [
        vec!["-Wcpp", "-Wno-cpp"],
        vec!["-Wno-cpp", "-Wno-error=cpp"],
    ] {
        let mut command = directory.command();
        command
            .args(["-E", "-P", "-nostdinc"])
            .args(&arguments)
            .arg(&source);
        let category_suppressed = run(command);
        category_suppressed.assert_success();
        assert!(
            !category_suppressed.stderr.contains("intentional warning"),
            "{arguments:?}: {}",
            category_suppressed.stderr
        );
    }

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-Werror=cpp"])
        .arg(source);
    let category_promoted = run(command);
    category_promoted.assert_failure();
    assert!(
        category_promoted.stderr.contains("intentional warning"),
        "{}",
        category_promoted.stderr
    );
}

#[test]
fn macro_redefinition_reports_the_previous_source_location() {
    let directory = support::TestWorkspace::new("preprocessing", "macro-redefinition-location")
        .retain_on_failure();
    let source = directory.write(
        "redefine.c",
        concat!(
            "#define F(a)a\n",
            "#define F(a) a\n",
            "#define VALUE 1\n",
            "#define VALUE 2\n",
            "VALUE\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(&source);
    let result = run(command);
    result.assert_success();

    assert_eq!(result.stderr.matches("macro 'VALUE' redefined").count(), 1);
    assert!(!result.stderr.contains("macro 'F' redefined"));
    assert!(
        result.stderr.contains(&format!("{}:4", source.display())),
        "missing redefinition location:\n{}",
        result.stderr
    );
    assert!(
        result.stderr.contains(&format!("{}:3", source.display())),
        "missing previous-definition location:\n{}",
        result.stderr
    );
    assert!(result.stderr.contains("previous definition"));
    assert!(!result.stderr.contains("file#"), "{}", result.stderr);
}

#[test]
fn warning_directives_remain_visible_in_system_header_regions() {
    let directory = support::TestWorkspace::new("preprocessing", "system-warning-directive")
        .retain_on_failure();
    let source = directory.write(
        "warning.i",
        concat!(
            "# 1 \"system-warning.h\" 3\n",
            "#warning visible system warning\n",
            "#pragma fixture_system_unknown\n",
            "int warning_sentinel;\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(&source);
    let default = run(command);
    default.assert_success();
    assert!(
        default.stderr.contains("visible system warning"),
        "{}",
        default.stderr
    );
    assert!(
        !default.stderr.contains("unknown pragma"),
        "{}",
        default.stderr
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-Wsystem-headers"])
        .arg(source);
    let enabled = run(command);
    enabled.assert_success();
    assert!(
        enabled.stderr.contains("visible system warning"),
        "{}",
        enabled.stderr
    );
    assert!(
        enabled.stderr.contains("unknown pragma"),
        "{}",
        enabled.stderr
    );
}

#[test]
fn pragma_once_system_headers_and_diagnostic_state_are_observable() {
    let directory = support::TestWorkspace::new("preprocessing", "pragmas").retain_on_failure();
    let source = directory.write(
        "main.c",
        concat!(
            "#include <system.h>\n",
            "#include <system.h>\n",
            "#pragma GCC diagnostic push\n",
            "#pragma GCC diagnostic ignored \"-Wunknown-pragmas\"\n",
            "#pragma fixture_suppressed\n",
            "#pragma GCC diagnostic pop\n",
            "#pragma fixture_visible\n",
            "int pragma_sentinel;\n",
        ),
    );
    let system_directory = directory.join("system");
    directory.write(
        "system/system.h",
        concat!(
            "#pragma once\n",
            "#pragma GCC system_header\n",
            "#pragma fixture_header_unknown\n",
            "int once_sentinel;\n",
        ),
    );

    let mut command = directory.command();
    command
        .args(["-E", "-nostdinc", "-isystem"])
        .arg(&system_directory)
        .arg(&source);
    let result = run(command);
    result.assert_success();
    assert_eq!(
        squash_whitespace(&result.stdout)
            .matches("intonce_sentinel;")
            .count(),
        1,
        "pragma once did not suppress the repeated include:\n{}",
        result.stdout
    );
    assert!(
        result.stdout.lines().any(|line| {
            line.starts_with("# ")
                && line.contains("\"system.h\"")
                && line.split_whitespace().any(|field| field == "3")
        }),
        "missing system-header linemarker flag:\n{}",
        result.stdout
    );
    assert_eq!(
        result.stderr.matches("unknown pragma").count(),
        1,
        "system-header or diagnostic suppression leaked:\n{}",
        result.stderr
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-Wsystem-headers", "-isystem"])
        .arg(system_directory)
        .arg(source);
    let system_warnings = run(command);
    system_warnings.assert_success();
    assert_eq!(
        system_warnings.stderr.matches("unknown pragma").count(),
        2,
        "-Wsystem-headers did not expose the header warning:\n{}",
        system_warnings.stderr
    );
}

#[test]
fn pragma_once_uses_physical_identity_across_hard_links() {
    let directory =
        support::TestWorkspace::new("preprocessing", "pragma-once-hard-link").retain_on_failure();
    let header = directory.write(
        "include/original.h",
        "#pragma once\nint physical_header_sentinel;\n",
    );
    let alias = directory.join("include/alias.h");
    fs::hard_link(&header, &alias).unwrap();
    let source = directory.write(
        "main.c",
        "#include <original.h>\n#include <alias.h>\nint main_sentinel;\n",
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-I"])
        .arg(directory.join("include"))
        .arg(source);
    let result = run(command);
    result.assert_success();
    assert_eq!(
        squash_whitespace(&result.stdout)
            .matches("intphysical_header_sentinel;")
            .count(),
        1,
        "{}",
        result.stdout
    );
}

#[test]
fn dependency_filtering_observes_a_system_header_pragma() {
    let directory = support::TestWorkspace::new("preprocessing", "pragma-system-dependency")
        .retain_on_failure();
    let header = directory.write(
        "include/reclassified.h",
        "#pragma GCC system_header\n#define RECLASSIFIED 42\n",
    );
    let source = directory.write(
        "main.c",
        "#include <reclassified.h>\nint reclassified = RECLASSIFIED;\n",
    );

    let mut command = directory.command();
    command
        .args(["-MM", "-nostdinc", "-I"])
        .arg(directory.join("include"))
        .arg(&source);
    let user_dependencies = run(command);
    user_dependencies.assert_success();
    assert!(
        !user_dependencies.stdout.contains(&make_quote_path(&header)),
        "{}",
        user_dependencies.stdout
    );

    let mut command = directory.command();
    command
        .args(["-M", "-nostdinc", "-I"])
        .arg(directory.join("include"))
        .arg(source);
    let all_dependencies = run(command);
    all_dependencies.assert_success();
    assert!(
        all_dependencies.stdout.contains(&make_quote_path(&header)),
        "{}",
        all_dependencies.stdout
    );
}

#[test]
fn dependency_modes_filter_quote_targets_and_emit_phony_rules() {
    let directory =
        support::TestWorkspace::new("preprocessing", "dependency-modes").retain_on_failure();
    let source = directory.write(
        "source dir/main.c",
        concat!(
            "#warning dependency-only warnings are suppressed by -M\n",
            "#include \"user header.h\"\n",
            "#include <system_header.h>\n",
            "int dependency_sentinel = USER_VALUE + SYSTEM_VALUE;\n",
        ),
    );
    let user = directory.join("user dir");
    let user_header = directory.write("user dir/user header.h", "#define USER_VALUE 1\n");
    let system = directory.join("system dir");
    let system_header = directory.write("system dir/system_header.h", "#define SYSTEM_VALUE 2\n");

    let mut command = directory.command();
    command
        .args(["-M", "-MP", "-MT", "$(objects)/main.o", "-MQ"])
        .arg("build dir/main$.o")
        .args(["-nostdinc", "-I"])
        .arg(&user)
        .arg("-isystem")
        .arg(&system)
        .arg(&source);
    let all = run(command);
    all.assert_success();
    assert!(all.stderr.trim().is_empty(), "{}", all.stderr);
    assert!(
        all.stdout
            .starts_with("$(objects)/main.o build\\ dir/main$$.o:"),
        "{}",
        all.stdout
    );
    for dependency in [&source, &user_header, &system_header] {
        assert!(
            all.stdout.contains(&make_quote_path(dependency)),
            "missing {} in:\n{}",
            dependency.display(),
            all.stdout
        );
    }
    assert!(
        all.stdout
            .contains(&format!("{}:\n", make_quote_path(&user_header))),
        "{}",
        all.stdout
    );
    assert!(
        all.stdout
            .contains(&format!("{}:\n", make_quote_path(&system_header))),
        "{}",
        all.stdout
    );
    assert!(
        all.stdout.contains(&format!(
            "\n\n{}:\n\n{}:\n",
            make_quote_path(&user_header),
            make_quote_path(&system_header)
        )),
        "phony rules were not separated:\n{}",
        all.stdout
    );
    assert!(!all.stdout.contains("dependency_sentinel"));

    let mut command = directory.command();
    command
        .args(["-MM", "-nostdinc", "-I"])
        .arg(&user)
        .arg("-isystem")
        .arg(&system)
        .arg(&source);
    let user_only = run(command);
    user_only.assert_success();
    assert!(user_only.stdout.contains(&make_quote_path(&user_header)));
    assert!(!user_only.stdout.contains(&make_quote_path(&system_header)));

    let all_file = directory.join("all.d");
    let mut command = directory.command();
    command
        .args(["-E", "-P", "-MD", "-MF"])
        .arg(&all_file)
        .args(["-nostdinc", "-w", "-I"])
        .arg(&user)
        .arg("-isystem")
        .arg(&system)
        .arg(&source);
    let side_effect_all = run(command);
    side_effect_all.assert_success();
    assert!(side_effect_all.stdout.contains("dependency_sentinel"));
    let all_dependencies = fs::read_to_string(all_file).unwrap();
    assert!(all_dependencies.contains(&make_quote_path(&system_header)));

    let user_file = directory.join("user.d");
    let mut command = directory.command();
    command
        .args(["-E", "-P", "-MMD", "-MF"])
        .arg(&user_file)
        .args(["-nostdinc", "-w", "-I"])
        .arg(user)
        .arg("-isystem")
        .arg(system)
        .arg(source);
    let side_effect_user = run(command);
    side_effect_user.assert_success();
    let user_dependencies = fs::read_to_string(user_file).unwrap();
    assert!(user_dependencies.contains(&make_quote_path(&user_header)));
    assert!(!user_dependencies.contains(&make_quote_path(&system_header)));
}

#[test]
fn failed_preprocessing_preserves_an_existing_dependency_file() {
    let directory =
        support::TestWorkspace::new("preprocessing", "dependency-failure").retain_on_failure();
    let source = directory.write("broken.c", "#include \"missing.h\"\n");
    let dependencies = directory.write("broken.d", "existing dependency contents\n");

    let mut command = directory.command();
    command
        .args(["-E", "-MMD", "-MF"])
        .arg(&dependencies)
        .arg("-nostdinc")
        .arg(source);
    let result = run(command);
    result.assert_failure();
    assert_eq!(
        fs::read_to_string(&dependencies).unwrap(),
        "existing dependency contents\n"
    );
    let pending_outputs = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".ccc-") && name.ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(
        pending_outputs.is_empty(),
        "temporary dependency outputs remain: {pending_outputs:?}"
    );
}

#[test]
fn forced_macro_files_precede_forced_includes_and_do_not_emit_text() {
    let directory =
        support::TestWorkspace::new("preprocessing", "forced-inputs").retain_on_failure();
    let first_macros = directory.write(
        "first-macros.h",
        "#define ORDER 1\nint imacros_text_must_not_be_emitted;\n",
    );
    let second_macros = directory.write("second-macros.h", "#undef ORDER\n#define ORDER 2\n");
    let forced_include = directory.write(
        "forced.h",
        concat!(
            "#if ORDER != 2\n",
            "#error forced include ran before the macro files\n",
            "#endif\n",
            "#define FORCED_VALUE 40\n",
            "int forced_include_text = ORDER;\n",
        ),
    );
    let source = directory.write("main.c", "int forced_result = FORCED_VALUE + ORDER;\n");

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-include"])
        .arg(forced_include)
        .arg("-imacros")
        .arg(first_macros)
        .arg("-imacros")
        .arg(second_macros)
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(!output.contains("imacros_text_must_not_be_emitted"));
    assert!(output.contains("intforced_include_text=2;"), "{output}");
    assert!(output.contains("intforced_result=40+2;"), "{output}");
}

#[test]
fn preprocesses_the_curated_hosted_header_tree_as_system_headers() {
    let include_directory = repository_fixture("test-corpus/libc-headers/glibc-like");
    let source = include_directory.join("probe.c");
    let directory =
        support::TestWorkspace::new("preprocessing", "curated-hosted-headers").retain_on_failure();

    for profile in ENABLED_TARGET_SPECS {
        let target = profile.triple.to_string();
        let mut command = directory.command_for_target(&target);
        command
            .args(["-E", "-P", "-nostdinc", "-isystem"])
            .arg(&include_directory)
            .arg(&source);
        let result = run(command);
        result.assert_success();
        assert!(
            result.stderr.trim().is_empty(),
            "{target}: {}",
            result.stderr
        );
        assert_eq!(
            normalize_fixture_snapshot(&result.stdout),
            include_str!("../../../tests/preprocessing/goldens/hosted-header.out"),
            "{target}"
        );
        let output = squash_whitespace(&result.stdout);
        assert!(
            output.contains("typedefunsignedlongintsize_t;"),
            "{target}: {output}"
        );
        assert!(
            output.contains("typedeflongintssize_t;"),
            "{target}: {output}"
        );
        assert!(
            output.contains("externssize_tfixture_read(int,void*__restrict,size_t)"),
            "{target}: {output}"
        );
        assert!(
            output.contains("inthosted_header_preprocessing_sentinel;"),
            "{target}: {output}"
        );
    }
}

#[test]
fn parses_the_curated_hosted_header_tree_as_system_headers() {
    let include_directory = repository_fixture("test-corpus/libc-headers/glibc-like");
    let source = include_directory.join("probe.c");
    let directory = support::TestWorkspace::new("preprocessing", "curated-hosted-header-parse")
        .retain_on_failure();

    for profile in ENABLED_TARGET_SPECS {
        let target = profile.triple.to_string();
        let mut command = directory.command_for_target(&target);
        command
            .args(["--dump-ast", "-nostdinc", "-isystem"])
            .arg(&include_directory)
            .arg(&source);
        let result = run(command);
        result.assert_success();
        assert!(
            result.stderr.trim().is_empty(),
            "{target}: {}",
            result.stderr
        );
        for sentinel in [
            "declarator fixture_record_t",
            "declarator fixture_read",
            "attribute __attribute__ __nothrow__",
            "asm-label __asm",
            "function-definition fixture_identity",
            "declarator hosted_header_preprocessing_sentinel",
        ] {
            assert!(
                result.stdout.contains(sentinel),
                "{target} AST dump is missing {sentinel:?}:\n{}",
                result.stdout
            );
        }
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn preprocesses_installed_target_glibc_headers() {
    let identity = support::installed_glibc_identity();
    eprintln!("hosted-header gate: {identity}");

    let directory =
        support::TestWorkspace::new("preprocessing", "installed-glibc-header").retain_on_failure();
    let source = directory.write(
        "installed.c",
        concat!(
            "#include <features.h>\n",
            "#include <stddef.h>\n",
            "#include <stdint.h>\n",
            "#ifndef __GLIBC__\n",
            "#error features.h did not identify glibc\n",
            "#endif\n",
            "#if !__GLIBC_PREREQ(2, 0)\n",
            "#error features.h exposed an invalid glibc version\n",
            "#endif\n",
            "int installed_glibc_major = __GLIBC__;\n",
            "int installed_glibc_minor = __GLIBC_MINOR__;\n",
            "size_t installed_size_type;\n",
            "intmax_t installed_intmax_type;\n",
            "uint64_t installed_uint64_type;\n",
            "int installed_glibc_header_sentinel = 1;\n",
        ),
    );

    let mut command = directory.command_for_target(support::native_linux_target_triple());
    command.args(["-E", "-P"]).arg(source);
    let result = run(command);
    support::assert_command_text_success(
        &format!("preprocess the installed hosted-header gate for {identity}"),
        &result.status,
        &result.stdout,
        &result.stderr,
        result.workspace.as_deref(),
    );
    let output = squash_whitespace(&result.stdout);
    assert!(
        output.contains("intinstalled_glibc_major=")
            && output.contains("intinstalled_glibc_minor=")
            && output.contains("size_tinstalled_size_type;")
            && output.contains("intmax_tinstalled_intmax_type;")
            && output.contains("uint64_tinstalled_uint64_type;")
            && output.contains("intinstalled_glibc_header_sentinel=1;"),
        "{}",
        result.stdout
    );
    for unexpanded in [
        "__SIZE_TYPE__",
        "__PTRDIFF_TYPE__",
        "__WCHAR_TYPE__",
        "__INTMAX_TYPE__",
    ] {
        assert!(
            !result.stdout.contains(unexpanded),
            "installed headers left {unexpanded} unexpanded:\n{}",
            result.stdout
        );
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn parses_installed_target_glibc_headers() {
    let identity = support::installed_glibc_identity();
    eprintln!("hosted-header gate: {identity}");
    let directory = support::TestWorkspace::new("preprocessing", "installed-glibc-header-parse")
        .retain_on_failure();
    let source = directory.write(
        "installed-parse.c",
        concat!(
            "#define _GNU_SOURCE 1\n",
            "#include <features.h>\n",
            "#include <stddef.h>\n",
            "#include <stdint.h>\n",
            "#include <sys/types.h>\n",
            "#include <unistd.h>\n",
            "#include <string.h>\n",
            "__extension__ typedef __typeof__(sizeof(0)) installed_typeof_sentinel_t;\n",
            "typedef __signed__ int *installed_pointer_sentinel_t;\n",
            "__restrict__ installed_pointer_sentinel_t installed_restrict_sentinel;\n",
            "extern __signed__ int installed_asm_sentinel(\n",
            "    __const__ char *__restrict__ value)\n",
            "    __asm__(\"installed_asm_target\") __attribute__((__nothrow__));\n",
            "static __inline__ __signed__ int installed_inline_sentinel(\n",
            "    __const__ __signed__ int *__restrict__ value) { return *value; }\n",
            "int installed_glibc_parse_sentinel;\n",
        ),
    );

    let mut command = directory.command_for_target(support::native_linux_target_triple());
    command.arg("--dump-ast").arg(source);
    let result = run(command);
    support::assert_command_text_success(
        &format!("parse the installed hosted-header gate for {identity}"),
        &result.status,
        &result.stdout,
        &result.stderr,
        result.workspace.as_deref(),
    );
    assert!(result.stderr.trim().is_empty(), "{}", result.stderr);

    let ast_lines = result.stdout.lines().map(str::trim).collect::<Vec<_>>();
    for declaration in [
        "declarator size_t",
        "declarator ssize_t",
        "declarator read(3)",
        "declarator *memcpy(3)",
        "declarator installed_typeof_sentinel_t",
        "declarator installed_restrict_sentinel",
        "declarator installed_asm_sentinel(1)",
        "asm-label __asm__ \"installed_asm_target\"",
        "attribute __attribute__ __nothrow__",
        "function-definition installed_inline_sentinel",
        "declarator installed_glibc_parse_sentinel",
    ] {
        assert!(
            ast_lines.contains(&declaration),
            "AST dump is missing exact line {declaration:?}:\n{}",
            result.stdout
        );
    }
    for syntax_surface in [
        "extension",
        "type Typeof",
        "qualifier Restrict",
        "function-specifier Inline",
    ] {
        assert!(
            ast_lines.contains(&syntax_surface),
            "AST dump is missing GNU declaration surface {syntax_surface:?}:\n{}",
            result.stdout
        );
    }
}

#[test]
fn recompiles_saved_preprocessor_output_with_numeric_linemarkers() {
    let directory = support::TestWorkspace::new("preprocessing", "saved-preprocessor-output")
        .retain_on_failure();
    let source = directory.write(
        "source/main.c",
        "#include \"value.h\"\nint saved_value(void) { return HEADER_VALUE; }\n",
    );
    directory.write("source/value.h", "#define HEADER_VALUE 42\n");
    let preprocessed = directory.join("build/main.i");
    fs::create_dir_all(preprocessed.parent().unwrap()).unwrap();

    let mut command = directory.command();
    command
        .args(["-E", "-nostdinc", "-o"])
        .arg(&preprocessed)
        .arg(source);
    let generated = run(command);
    generated.assert_success();
    assert!(generated.stdout.is_empty(), "{}", generated.stdout);
    let saved = fs::read_to_string(&preprocessed).unwrap();
    assert!(
        saved.lines().any(|line| line.starts_with("# 1 \"")),
        "saved output has no numeric linemarker:\n{saved}"
    );
    assert!(
        squash_whitespace(&saved).contains("intsaved_value(void){return42;}"),
        "{saved}"
    );

    let object = directory.join("build/main.o");
    let mut command = directory.command();
    command
        .args(["-c", "-nostdinc", "-o"])
        .arg(&object)
        .arg(&preprocessed);
    let compiled = run(command);
    compiled.assert_success();
    assert!(
        fs::metadata(object).unwrap().len() > 0,
        "recompilation produced an empty object"
    );
}

#[test]
fn preprocessed_c_inputs_are_not_macro_expanded_again() {
    let directory =
        support::TestWorkspace::new("preprocessing", "preprocessed-c-language").retain_on_failure();
    for (name, language) in [
        ("implicit.i", None),
        ("extensionless", Some("c-cpp-output")),
    ] {
        let source = directory.write(name, "int SELECTED(void) { return 42; }\n");
        let object_path = directory.join(format!("{name}.o"));
        let mut command = directory.command();
        command.args(["-c", "-nostdinc", "-DSELECTED=reexpanded"]);
        if let Some(language) = language {
            command.args(["-x", language]);
        }
        command.arg(&source).arg("-o").arg(&object_path);
        let compiled = run(command);
        compiled.assert_success();

        let bytes = fs::read(&object_path).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let symbols = object
            .symbols()
            .filter_map(|symbol| symbol.name().ok())
            .collect::<Vec<_>>();
        assert!(symbols.contains(&"SELECTED"), "{name}: {symbols:?}");
        assert!(!symbols.contains(&"reexpanded"), "{name}: {symbols:?}");
    }
}

#[test]
fn stringization_preserves_one_separator_between_argument_tokens() {
    let directory =
        support::TestWorkspace::new("preprocessing", "exact-stringization").retain_on_failure();
    let source = directory.write(
        "stringize.c",
        concat!(
            "#define STRINGIZE(value) #value\n",
            "const char *operators = STRINGIZE(alpha   +\t beta);\n",
            "const char *comment = STRINGIZE(alpha /* separator */ beta);\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        result.stdout.contains("\"alpha + beta\""),
        "{}",
        result.stdout
    );
    assert!(
        result.stdout.contains("\"alpha beta\""),
        "{}",
        result.stdout
    );
    assert!(
        !result.stdout.contains("\"alpha+beta\""),
        "{}",
        result.stdout
    );
    assert!(
        !result.stdout.contains("\"alphabeta\""),
        "{}",
        result.stdout
    );
}

#[test]
fn expands_function_macros_with_multiline_invocations_and_arguments() {
    let directory = support::TestWorkspace::new("preprocessing", "multiline-macro-invocation")
        .retain_on_failure();
    let source = directory.write(
        "multiline.c",
        concat!(
            "#define ADD(left, right) ((left) + (right))\n",
            "#define SECOND(first, second) second\n",
            "int multiline_value = SECOND(\n",
            "    ignored,\n",
            "    ADD(\n",
            "        19,\n",
            "        23\n",
            "    )\n",
            ");\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intmultiline_value=((19)+(23));"),
        "{}",
        result.stdout
    );
}

#[test]
fn token_pasting_treats_empty_operands_as_placemarkers() {
    let directory =
        support::TestWorkspace::new("preprocessing", "empty-paste-operands").retain_on_failure();
    let source = directory.write(
        "paste.c",
        concat!(
            "#define CAT(left, right) left ## right\n",
            "int CAT(, left_name) = 1;\n",
            "int CAT(right_name, ) = 2;\n",
            "int both_empty = 1 CAT(, ) + 2;\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("intleft_name=1;"), "{output}");
    assert!(output.contains("intright_name=2;"), "{output}");
    assert!(output.contains("intboth_empty=1+2;"), "{output}");
}

#[test]
fn expands_computed_includes_but_not_direct_angle_header_names() {
    let directory = support::TestWorkspace::new("preprocessing", "direct-and-computed-includes")
        .retain_on_failure();
    let source = directory.write(
        "main.c",
        concat!(
            "#define direct redirected\n",
            "#include <direct.h>\n",
            "#define COMPUTED_HEADER <computed.h>\n",
            "#include COMPUTED_HEADER\n",
            "int include_value = DIRECT_VALUE + COMPUTED_VALUE;\n",
        ),
    );
    let include = directory.join("include");
    directory.write("include/direct.h", "#define DIRECT_VALUE 1\n");
    directory.write("include/redirected.h", "#define DIRECT_VALUE 99\n");
    directory.write("include/computed.h", "#define COMPUTED_VALUE 2\n");

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-I"])
        .arg(include)
        .arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intinclude_value=1+2;"),
        "{}",
        result.stdout
    );
}

#[test]
fn include_next_resumes_after_the_directory_that_found_the_current_header() {
    let directory =
        support::TestWorkspace::new("preprocessing", "include-next").retain_on_failure();
    let source = directory.write(
        "main.c",
        "#include <chain.h>\nint chained_value = FIRST_VALUE + SECOND_VALUE;\n",
    );
    let first = directory.join("first");
    directory.write(
        "first/chain.h",
        concat!(
            "#ifndef FIRST_CHAIN_H\n",
            "#define FIRST_CHAIN_H\n",
            "#define FIRST_VALUE 1\n",
            "#include_next <chain.h>\n",
            "#endif\n",
        ),
    );
    let unrelated = directory.join("unrelated");
    fs::create_dir_all(&unrelated).unwrap();
    let second = directory.join("second");
    directory.write("second/chain.h", "#define SECOND_VALUE 2\n");

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-I"])
        .arg(&first)
        .arg("-I")
        .arg(&unrelated)
        .arg("-I")
        .arg(&first)
        .arg("-I")
        .arg(second)
        .arg(&source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intchained_value=1+2;"),
        "{}",
        result.stdout
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-std=c11", "-nostdinc", "-I"])
        .arg(directory.join("first"))
        .arg("-I")
        .arg(directory.join("unrelated"))
        .arg("-I")
        .arg(directory.join("first"))
        .arg("-I")
        .arg(directory.join("second"))
        .arg(source);
    let strict = run(command);
    strict.assert_success();
    assert!(
        squash_whitespace(&strict.stdout).contains("intchained_value=1+2;"),
        "{}",
        strict.stdout
    );
}

#[test]
fn forced_include_names_are_resolved_through_user_include_paths() {
    let directory =
        support::TestWorkspace::new("preprocessing", "forced-include-search").retain_on_failure();
    directory.write("headers/forced.h", "#define FORCED_SEARCH_VALUE 42\n");
    directory.write("source/forced.h", "#define FORCED_SEARCH_VALUE 1\n");
    directory.write(
        "source/main.c",
        "int forced_search_value = FORCED_SEARCH_VALUE;\n",
    );

    let mut command = directory.command();
    command.args([
        "-E",
        "-P",
        "-nostdinc",
        "-I",
        "headers",
        "-include",
        "forced.h",
        "source/main.c",
    ]);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intforced_search_value=42;"),
        "{}",
        result.stdout
    );
}

#[test]
fn preprocessing_token_dump_has_stable_macro_origin_summaries() {
    let directory =
        support::TestWorkspace::new("preprocessing", "pp-token-origins").retain_on_failure();
    let source = directory.write(
        "origins.c",
        concat!(
            "#define OBJECT 42\n",
            "#define ID(value) value\n",
            "#define CAT(left, right) left ## right\n",
            "#define STRINGIZE(value) #value\n",
            "int object_value = ID(OBJECT);\n",
            "int CAT(pas, ted) = 0;\n",
            "const char *text = STRINGIZE(alpha beta);\n",
        ),
    );

    let mut command = directory.command();
    command.args(["--dump-pp-tokens", "-nostdinc"]).arg(&source);
    let result = run(command);
    result.assert_success();
    let expanded = result
        .stdout
        .lines()
        .filter(|line| !line.ends_with("origin=direct"))
        .collect::<Vec<_>>();
    assert_eq!(expanded.len(), 3, "{}", result.stdout);
    let summaries = expanded
        .iter()
        .map(|line| {
            let (prefix, origin) = line.rsplit_once(" origin=").unwrap();
            let location_start = prefix
                .find(&format!(" {}", source.display()))
                .expect("token dump contains the input display path");
            let kind_and_spelling = &prefix[..location_start];
            format!("{kind_and_spelling} origin={origin}")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summaries,
        [
            "pp-number \"42\" origin=argument:value>macro:OBJECT",
            "identifier \"pasted\" origin=paste>argument:left",
            "string-literal \"\\\"alpha beta\\\"\" origin=stringize>macro:STRINGIZE",
        ],
        "{}",
        result.stdout
    );
}

#[test]
fn normalizes_bom_crlf_splices_and_valid_universal_character_names() {
    let directory =
        support::TestWorkspace::new("preprocessing", "source-normalization").retain_on_failure();
    let source = directory.write(
        "normalized.c",
        concat!(
            "\u{feff}#define JOINED 4\\\r\n",
            "2\r\n",
            "#define caf\\u00e9 9\r\n",
            "int normalized_value = JOINED + café;\r\n",
        ),
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(source);
    let result = run(command);
    result.assert_success();
    assert!(
        squash_whitespace(&result.stdout).contains("intnormalized_value=42+9;"),
        "{}",
        result.stdout
    );
}

#[test]
fn language_mode_controls_default_trigraph_conversion() {
    let directory = support::TestWorkspace::new("preprocessing", "language-trigraph-defaults")
        .retain_on_failure();
    let source = directory.write(
        "trigraph.c",
        "??=define TRIGRAPH_VALUE 42\nint trigraph_value = TRIGRAPH_VALUE;\n",
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-std=gnu11"])
        .arg(&source);
    let gnu = run(command);
    gnu.assert_success();
    assert!(
        squash_whitespace(&gnu.stdout).contains("inttrigraph_value=TRIGRAPH_VALUE;"),
        "{}",
        gnu.stdout
    );

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-std=c11"])
        .arg(source);
    let strict = run(command);
    strict.assert_success();
    assert!(
        squash_whitespace(&strict.stdout).contains("inttrigraph_value=42;"),
        "{}",
        strict.stdout
    );
}

#[test]
fn c99_build_profile_aliases_retain_the_c11_macro_identity() {
    let directory = support::TestWorkspace::new("preprocessing", "c99-alias-macro-identity")
        .retain_on_failure();
    let source = directory.write("identity.c", "int identity;\n");

    for (mode, strict) in [("-std=gnu99", false), ("-std=c99", true)] {
        let mut command = directory.command();
        command.args(["-dM", "-E", "-nostdinc", mode]).arg(&source);
        let macros = run(command);
        macros.assert_success();
        assert!(
            macros
                .stdout
                .lines()
                .any(|line| line == "#define __STDC_VERSION__ 201112L"),
            "{mode}: {}",
            macros.stdout
        );
        assert_eq!(
            macros
                .stdout
                .lines()
                .any(|line| line == "#define __STRICT_ANSI__ 1"),
            strict,
            "{mode}: {}",
            macros.stdout
        );
    }
}

#[test]
fn discovers_and_preprocesses_compiler_resource_headers() {
    let directory = support::TestWorkspace::new("preprocessing", "compiler-resource-headers")
        .retain_on_failure();
    let source = directory.write(
        "resources.c",
        concat!(
            "#include <stdbool.h>\n",
            "#include <stdalign.h>\n",
            "#include <stdnoreturn.h>\n",
            "bool resource_flag = true;\n",
            "alignas(8) int resource_aligned;\n",
            "noreturn void resource_exit(void);\n",
            "int resource_false = false;\n",
        ),
    );
    let resources = repository_fixture("resource-dir");

    let mut command = directory.host_command();
    command
        .args(["-E", "-P", "-resource-dir"])
        .arg(resources)
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("_Boolresource_flag=1;"), "{output}");
    assert!(
        output.contains("_Alignas(8)intresource_aligned;"),
        "{output}"
    );
    assert!(
        output.contains("_Noreturnvoidresource_exit(void);"),
        "{output}"
    );
    assert!(output.contains("intresource_false=0;"), "{output}");
}

#[test]
fn float_resource_header_exposes_binary16_limits_on_request() {
    let directory =
        support::TestWorkspace::new("preprocessing", "float16-resource-header").retain_on_failure();
    let source = directory.write(
        "float16.c",
        concat!(
            "#define __STDC_WANT_IEC_60559_TYPES_EXT__ 1\n",
            "#include <float.h>\n",
            "#if FLT16_MANT_DIG != 11 || FLT16_MAX_EXP != 16\n",
            "#error binary16 precision facts are unavailable\n",
            "#endif\n",
            "_Float16 maximum = FLT16_MAX;\n",
            "_Float16 epsilon = FLT16_EPSILON;\n",
            "_Float16 true_minimum = FLT16_TRUE_MIN;\n",
        ),
    );
    let resources = repository_fixture("resource-dir");

    let mut command = directory.host_command();
    command
        .args(["-E", "-P", "-resource-dir"])
        .arg(resources)
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(output.contains("_Float16maximum="), "{output}");
    assert!(output.contains("_Float16epsilon="), "{output}");
    assert!(output.contains("_Float16true_minimum="), "{output}");
}

#[test]
fn stdatomic_resource_header_exposes_native_scalar_operations_without_overclaiming() {
    let directory = support::TestWorkspace::new("preprocessing", "stdatomic-resource-header")
        .retain_on_failure();
    let source = directory.write(
        "stdatomic.c",
        concat!(
            "#include <stdatomic.h>\n",
            "#ifndef __STDC_NO_ATOMICS__\n",
            "#error partial scalar atomics must not claim every atomic type\n",
            "#endif\n",
            "#if !__has_builtin(__atomic_load_n)\n",
            "#error scalar atomic load builtin is unavailable\n",
            "#endif\n",
            "atomic_int value = ATOMIC_VAR_INIT(3);\n",
            "int read(void) { return atomic_load_explicit(&value, memory_order_relaxed); }\n",
            "int lock_free = ATOMIC_INT_LOCK_FREE;\n",
        ),
    );
    let resources = repository_fixture("resource-dir");

    let mut command = directory.host_command();
    command
        .args(["-E", "-P", "-resource-dir"])
        .arg(resources)
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert!(
        output.contains("typedef_Atomic(int)atomic_int;"),
        "{output}"
    );
    assert!(
        output.contains("return__atomic_load_n((&value),(memory_order_relaxed));"),
        "{output}"
    );
    assert!(output.contains("intlock_free=2;"), "{output}");
}

#[test]
fn stdarg_resource_header_supports_repeated_and_partial_inclusion() {
    let directory =
        support::TestWorkspace::new("preprocessing", "stdarg-resource-header").retain_on_failure();
    let source = directory.write(
        "stdarg.c",
        concat!(
            "#if !__has_builtin(__builtin_va_arg)\n",
            "#error variadic builtin registry is unavailable\n",
            "#endif\n",
            "#define __need___va_list\n",
            "#include <stdarg.h>\n",
            "__gnuc_va_list first;\n",
            "#define __need_va_list\n",
            "#include <stdarg.h>\n",
            "va_list second;\n",
            "#define __need_va_arg\n",
            "#define __need_va_copy\n",
            "#include <stdarg.h>\n",
            "#include <stdarg.h>\n",
            "int consume(int marker, ...) {\n",
            "    va_list list;\n",
            "    va_start(list, marker);\n",
            "    va_copy(second, list);\n",
            "    marker = va_arg(list, int);\n",
            "    va_end(list);\n",
            "    return marker;\n",
            "}\n",
        ),
    );
    let resources = repository_fixture("resource-dir");

    let mut command = directory.host_command();
    command
        .args(["-E", "-P", "-resource-dir"])
        .arg(resources)
        .arg(source);
    let result = run(command);
    result.assert_success();
    let output = squash_whitespace(&result.stdout);
    assert_eq!(
        output
            .matches("typedef__builtin_va_list__gnuc_va_list;")
            .count(),
        1
    );
    assert_eq!(
        output.matches("typedef__builtin_va_listva_list;").count(),
        1
    );
    assert!(
        output.contains("__builtin_va_start(list,marker)"),
        "{output}"
    );
    assert!(
        output.contains("__builtin_va_copy(second,list)"),
        "{output}"
    );
    assert!(output.contains("__builtin_va_arg(list,int)"), "{output}");
    assert!(output.contains("__builtin_va_end(list)"), "{output}");
}

#[test]
fn nobuiltininc_removes_the_stdarg_resource_header() {
    let directory =
        support::TestWorkspace::new("preprocessing", "stdarg-nobuiltininc").retain_on_failure();
    let source = directory.write("stdarg.c", "#include <stdarg.h>\n");
    let resources = repository_fixture("resource-dir");

    let mut command = directory.command();
    command
        .args(["-E", "-P", "-nostdinc", "-nobuiltininc", "-resource-dir"])
        .arg(resources)
        .arg(source);
    let result = run(command);
    result.assert_failure();
    assert!(result.stderr.contains("stdarg.h"), "{}", result.stderr);
}

#[test]
fn dependency_rules_preserve_default_and_explicit_relative_spellings() {
    let directory = support::TestWorkspace::new("preprocessing", "relative-dependency-spelling")
        .retain_on_failure();
    directory.write(
        "src/main.c",
        "#include <value.h>\nint dependency_value = VALUE;\n",
    );
    directory.write("headers/value.h", "#define VALUE 42\n");

    let mut command = directory.command();
    command.args(["-M", "-nostdinc", "-I", "headers", "src/main.c"]);
    let default = run(command);
    default.assert_success();
    assert_eq!(
        default.stdout, "main.o: src/main.c headers/value.h\n",
        "default dependency spelling changed"
    );

    fs::create_dir_all(directory.join("deps")).unwrap();
    let mut command = directory.command();
    command.args([
        "-M",
        "-MF",
        "deps/custom.d",
        "-MT",
        "objects/custom.o",
        "-nostdinc",
        "-I",
        "headers",
        "src/main.c",
    ]);
    let explicit = run(command);
    explicit.assert_success();
    assert!(explicit.stdout.is_empty(), "{}", explicit.stdout);
    assert_eq!(
        fs::read_to_string(directory.join("deps/custom.d")).unwrap(),
        "objects/custom.o: src/main.c headers/value.h\n",
        "explicit dependency spelling changed"
    );
}

#[test]
fn inactive_token_validation_matches_an_available_reference_preprocessor() {
    let directory = support::TestWorkspace::new("preprocessing", "reference-inactive-tokens")
        .retain_on_failure();
    let probe = directory.write("probe.c", "int reference_probe;\n");
    let Some(probe_result) = run_reference_preprocessor(&directory, &probe) else {
        return;
    };
    if !probe_result.status.success() {
        return;
    }

    let valid = directory.write(
        "valid.c",
        "#if 0\n} else ) ] this is token-valid but not C syntax\n#endif\nint live_value;\n",
    );
    let malformed = directory.write("malformed.c", "#if 0\n/* unterminated token\n#endif\n");
    let (Some(reference_valid), Some(reference_malformed)) = (
        run_reference_preprocessor(&directory, &valid),
        run_reference_preprocessor(&directory, &malformed),
    ) else {
        return;
    };
    if !reference_valid.status.success() || reference_malformed.status.success() {
        return;
    }

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(valid);
    let ccc_valid = run(command);
    assert_eq!(
        ccc_valid.status.success(),
        reference_valid.status.success(),
        "valid inactive tokens differ from cc\nccc stderr:\n{}\ncc stderr:\n{}",
        ccc_valid.stderr,
        reference_valid.stderr
    );
    assert!(
        squash_whitespace(&ccc_valid.stdout).contains("intlive_value;"),
        "{}",
        ccc_valid.stdout
    );

    let mut command = directory.command();
    command.args(["-E", "-P", "-nostdinc"]).arg(malformed);
    let ccc_malformed = run(command);
    assert_eq!(
        ccc_malformed.status.success(),
        reference_malformed.status.success(),
        "malformed inactive token handling differs from cc\nccc stderr:\n{}\ncc stderr:\n{}",
        ccc_malformed.stderr,
        reference_malformed.stderr
    );
}
