#![cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    feature = "ci-sysv-amd64"
))]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use object::read::{Object as _, ObjectSection as _, ObjectSymbol as _};
use object::{RelocationTarget, SectionFlags, SymbolScope};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str, compiler: &str, optimization: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ccc-sysv-amd64-{}-{}-{name}-{compiler}-{}",
        std::process::id(),
        TEST_ID.fetch_add(1, Ordering::Relaxed),
        optimization.trim_start_matches('-')
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn run(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to execute {rendered}: {error}"));
    assert!(
        output.status.success(),
        "command failed: {rendered}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn reference_compilers() -> [&'static str; 2] {
    ["gcc", "clang"]
}

fn reference_configurations() -> impl Iterator<Item = (&'static str, &'static str)> {
    reference_compilers().into_iter().flat_map(|compiler| {
        ["-O0", "-O2"]
            .into_iter()
            .map(move |level| (compiler, level))
    })
}

fn compile_ccc(compiler: &str, source: &Path, object: &Path) {
    let staging_directory = object.parent().unwrap().join(format!(
        "ccc-stage-{}",
        TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&staging_directory).unwrap();
    let staged_object = staging_directory.join("translation-unit.o");
    run(Command::new(env!("CARGO_BIN_EXE_ccc"))
        .env("CCC_CC", compiler)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(&staged_object));
    fs::rename(&staged_object, object).unwrap();
    fs::remove_dir_all(&staging_directory).unwrap();
}

fn compile_reference(compiler: &str, optimization: &str, source: &Path, object: &Path) {
    run(Command::new(compiler)
        .args([
            "-std=c11",
            optimization,
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
        ])
        .arg(source)
        .arg("-o")
        .arg(object));
}

fn link_and_run(compiler: &str, directory: &Path, objects: &[&Path]) -> Output {
    let executable = directory.join("program");
    let mut command = Command::new(compiler);
    command.arg("-no-pie");
    command.args(objects.iter().map(|path| path.as_os_str()));
    command.arg("-o").arg(&executable);
    run(&mut command);
    run(Command::new(executable).env("LC_ALL", "C"))
}

fn bridge_generated_symbol(name: &str) -> bool {
    [
        "__ccc_call_helper_",
        "__ccc_call_stub_",
        "__ccc_variadic_body_",
        "__ccc_support_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn corpus_argument_counts(case: &ccc_abi::CorpusCase) -> (usize, usize, usize, usize) {
    (
        usize::from(case.allocation.leading_integer),
        usize::from(case.allocation.leading_sse),
        usize::from(case.allocation.trailing_integer),
        usize::from(case.allocation.trailing_sse),
    )
}

fn corpus_signature(
    case: &ccc_abi::CorpusCase,
    tag: &str,
    function: &str,
    parameter_names: bool,
) -> String {
    let (leading_gp, leading_sse, trailing_gp, trailing_sse) = corpus_argument_counts(case);
    let mut parameters =
        Vec::with_capacity(leading_gp + leading_sse + 1 + trailing_gp + trailing_sse);
    for index in 0..leading_gp {
        parameters.push(if parameter_names {
            format!("long leading_gp{index}")
        } else {
            "long".to_owned()
        });
    }
    for index in 0..leading_sse {
        parameters.push(if parameter_names {
            format!("double leading_sse{index}")
        } else {
            "double".to_owned()
        });
    }
    parameters.push(if parameter_names {
        format!("{} value", case.c_type_name(tag))
    } else {
        case.c_type_name(tag)
    });
    for index in 0..trailing_gp {
        parameters.push(if parameter_names {
            format!("long trailing_gp{index}")
        } else {
            "long".to_owned()
        });
    }
    for index in 0..trailing_sse {
        parameters.push(if parameter_names {
            format!("double trailing_sse{index}")
        } else {
            "double".to_owned()
        });
    }
    format!(
        "{} {function}({})",
        case.c_type_name(tag),
        parameters.join(", ")
    )
}

fn corpus_call_arguments(case: &ccc_abi::CorpusCase) -> (String, i64) {
    let (leading_gp, leading_sse, trailing_gp, trailing_sse) = corpus_argument_counts(case);
    let mut arguments = vec!["1".to_owned(); leading_gp];
    arguments.extend(vec!["1.0".to_owned(); leading_sse]);
    arguments.push("value".to_owned());
    arguments.extend(vec!["1".to_owned(); trailing_gp]);
    arguments.extend(vec!["1.0".to_owned(); trailing_sse]);
    let delta = i64::try_from(1 + leading_gp + leading_sse + trailing_gp + trailing_sse).unwrap();
    (arguments.join(", "), delta)
}

fn corpus_delta_body(case: &ccc_abi::CorpusCase) -> String {
    let (leading_gp, leading_sse, trailing_gp, trailing_sse) = corpus_argument_counts(case);
    let mut body = "    long delta = 1;\n".to_owned();
    for index in 0..leading_gp {
        writeln!(body, "    delta += leading_gp{index};").unwrap();
    }
    for index in 0..leading_sse {
        writeln!(body, "    delta += (long)leading_sse{index};").unwrap();
    }
    for index in 0..trailing_gp {
        writeln!(body, "    delta += trailing_gp{index};").unwrap();
    }
    for index in 0..trailing_sse {
        writeln!(body, "    delta += (long)trailing_sse{index};").unwrap();
    }
    body
}

fn corpus_operations(fixture: ccc_abi::CorpusFixture, delta: i64) -> (String, String, String) {
    match fixture {
        ccc_abi::CorpusFixture::IntegerBytes(length) => {
            let initialize = format!(
                "        int index;\n        for (index = 0; index < {length}; ++index) value.bytes[index] = 0;\n        value.bytes[0] = 3;\n{}",
                if length > 1 {
                    format!("        value.bytes[{}] = 5;\n", length - 1)
                } else {
                    String::new()
                }
            );
            let check = if length > 1 {
                format!(
                    "result.bytes[0] == {} && result.bytes[{}] == 5",
                    3 + delta,
                    length - 1
                )
            } else {
                format!("result.bytes[0] == {}", 3 + delta)
            };
            (
                initialize,
                "    value.bytes[0] = (char)(value.bytes[0] + delta);\n".to_owned(),
                check,
            )
        }
        ccc_abi::CorpusFixture::FloatRecord => (
            "        value.value = 1.5f;\n".to_owned(),
            "    value.value += (float)delta;\n".to_owned(),
            format!("result.value == {}.5f", 1 + delta),
        ),
        ccc_abi::CorpusFixture::DoubleArray(length) => {
            let initialize = format!(
                "        int index;\n        for (index = 0; index < {length}; ++index) value.values[index] = 0.0;\n        value.values[0] = 1.5;\n{}",
                if length > 1 {
                    format!("        value.values[{}] = 2.5;\n", length - 1)
                } else {
                    String::new()
                }
            );
            let check = if length > 1 {
                format!(
                    "result.values[0] == {}.5 && result.values[{}] == 2.5",
                    1 + delta,
                    length - 1
                )
            } else {
                format!("result.values[0] == {}.5", 1 + delta)
            };
            (
                initialize,
                "    value.values[0] += (double)delta;\n".to_owned(),
                check,
            )
        }
        ccc_abi::CorpusFixture::IntegerPair => (
            "        value.first = 3; value.second = 5;\n".to_owned(),
            "    value.first += delta;\n".to_owned(),
            format!("result.first == {} && result.second == 5", 3 + delta),
        ),
        ccc_abi::CorpusFixture::MixedSseInteger
        | ccc_abi::CorpusFixture::MixedIntegerSse => (
            "        value.integer = 3; value.floating = 2.5;\n".to_owned(),
            "    value.integer += delta;\n".to_owned(),
            format!("result.integer == {} && result.floating == 2.5", 3 + delta),
        ),
        ccc_abi::CorpusFixture::UnionMerge => (
            "        value.integer = 3;\n".to_owned(),
            "    value.integer += delta;\n".to_owned(),
            format!("result.integer == {}", 3 + delta),
        ),
        ccc_abi::CorpusFixture::PackedAlignedNine => (
            "        value.integer = 3; value.tail = 5;\n".to_owned(),
            "    value.integer += delta;\n".to_owned(),
            format!("result.integer == {} && result.tail == 5", 3 + delta),
        ),
        ccc_abi::CorpusFixture::PackedUnalignedInteger => (
            "        value.prefix = 3; value.integer = 5;\n".to_owned(),
            "    value.integer += (int)delta;\n".to_owned(),
            format!("result.prefix == 3 && result.integer == {}", 5 + delta),
        ),
        ccc_abi::CorpusFixture::CrossingBitfield => (
            "        int index;\n        for (index = 0; index < 7; ++index) value.prefix[index] = 0;\n        value.prefix[0] = 3; value.prefix[6] = 5; value.bits = 7;\n"
                .to_owned(),
            "    value.bits += (unsigned long)delta;\n".to_owned(),
            format!(
                "result.prefix[0] == 3 && result.prefix[6] == 5 && result.bits == {}",
                7 + delta
            ),
        ),
        ccc_abi::CorpusFixture::NestedUnionAndDouble => (
            "        value.nested.integer = 3; value.tail = 2.5;\n".to_owned(),
            "    value.nested.integer += delta;\n".to_owned(),
            format!(
                "result.nested.integer == {} && result.tail == 2.5",
                3 + delta
            ),
        ),
    }
}

fn corpus_definitions(cases: &[(usize, &ccc_abi::CorpusCase)], prefix: &str) -> String {
    let mut source = String::new();
    for (index, case) in cases {
        let tag = format!("Corpus{index:03}");
        writeln!(source, "{}", case.c_declaration(&tag)).unwrap();
    }
    for (index, case) in cases {
        let tag = format!("Corpus{index:03}");
        let function = format!("{prefix}_{index:03}");
        writeln!(
            source,
            "{} {{",
            corpus_signature(case, &tag, &function, true)
        )
        .unwrap();
        source.push_str(&corpus_delta_body(case));
        let (_, mutation, _) = corpus_operations(case.fixture, 0);
        source.push_str(&mutation);
        source.push_str("    return value;\n}\n");
    }
    source
}

fn corpus_caller(
    cases: &[(usize, &ccc_abi::CorpusCase)],
    prefix: &str,
    entry: &str,
    with_main: bool,
) -> String {
    let mut source = String::new();
    for (index, case) in cases {
        let tag = format!("Corpus{index:03}");
        writeln!(source, "{}", case.c_declaration(&tag)).unwrap();
    }
    for (index, case) in cases {
        let tag = format!("Corpus{index:03}");
        let function = format!("{prefix}_{index:03}");
        writeln!(
            source,
            "{};",
            corpus_signature(case, &tag, &function, false)
        )
        .unwrap();
    }
    writeln!(source, "int {entry}(void) {{").unwrap();
    for (index, case) in cases {
        let tag = format!("Corpus{index:03}");
        let ty = case.c_type_name(&tag);
        let function = format!("{prefix}_{index:03}");
        let (arguments, delta) = corpus_call_arguments(case);
        let (initialize, _, check) = corpus_operations(case.fixture, delta);
        source.push_str("    {\n");
        writeln!(source, "        {ty} value;").unwrap();
        writeln!(source, "        {ty} result;").unwrap();
        source.push_str(&initialize);
        writeln!(source, "        result = {function}({arguments});").unwrap();
        writeln!(
            source,
            "        if (!({check})) return {};",
            index % 254 + 1
        )
        .unwrap();
        source.push_str("    }\n");
    }
    source.push_str("    return 0;\n}\n");
    if with_main {
        writeln!(source, "int main(void) {{ return {entry}(); }}").unwrap();
    }
    source
}

#[test]
fn deterministic_classifier_corpus_cross_links_with_gcc_and_clang() {
    let selected = ccc_abi::selected_cross_link_cases();
    assert_eq!(selected.len(), 256);
    let expected = (0..selected.len()).collect::<BTreeSet<_>>();
    let mut compiler_coverage = BTreeMap::<&str, BTreeSet<usize>>::new();

    // Each reference compiler sees every selected case in both directions.
    // Even cases use -O0 and odd cases use -O2, keeping each translation unit
    // bounded while the complete gate covers both optimization profiles.
    for compiler in reference_compilers() {
        for (parity, optimization) in [(0, "-O0"), (1, "-O2")] {
            let cases = selected
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == parity)
                .collect::<Vec<_>>();
            assert_eq!(cases.len(), 128);
            compiler_coverage
                .entry(compiler)
                .or_default()
                .extend(cases.iter().map(|(index, _)| *index));

            let directory = test_directory("classifier-corpus", compiler, optimization);
            let ccc_caller_source =
                corpus_caller(&cases, "reference_corpus", "ccc_corpus_entry", false);
            let mut reference_callee_source = corpus_definitions(&cases, "reference_corpus");
            reference_callee_source.push_str(
                "int ccc_corpus_entry(void);\nint main(void) { return ccc_corpus_entry(); }\n",
            );
            let ccc_caller = write(&directory, "ccc-corpus-caller.c", &ccc_caller_source);
            let reference_callee = write(
                &directory,
                "reference-corpus-callee.c",
                &reference_callee_source,
            );
            let ccc_caller_object = directory.join("ccc-corpus-caller.o");
            let reference_callee_object = directory.join("reference-corpus-callee.o");
            compile_ccc(compiler, &ccc_caller, &ccc_caller_object);
            compile_reference(
                compiler,
                optimization,
                &reference_callee,
                &reference_callee_object,
            );
            let output = link_and_run(
                compiler,
                &directory,
                &[&ccc_caller_object, &reference_callee_object],
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());

            let ccc_callee_source = corpus_definitions(&cases, "ccc_corpus");
            let reference_caller_source =
                corpus_caller(&cases, "ccc_corpus", "reference_corpus_entry", true);
            let ccc_callee = write(&directory, "ccc-corpus-callee.c", &ccc_callee_source);
            let reference_caller = write(
                &directory,
                "reference-corpus-caller.c",
                &reference_caller_source,
            );
            let ccc_callee_object = directory.join("ccc-corpus-callee.o");
            let reference_caller_object = directory.join("reference-corpus-caller.o");
            compile_ccc(compiler, &ccc_callee, &ccc_callee_object);
            compile_reference(
                compiler,
                optimization,
                &reference_caller,
                &reference_caller_object,
            );
            let output = link_and_run(
                compiler,
                &directory,
                &[&ccc_callee_object, &reference_caller_object],
            );
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
            fs::remove_dir_all(directory).unwrap();
        }
    }

    assert_eq!(compiler_coverage.get("gcc"), Some(&expected));
    assert_eq!(compiler_coverage.get("clang"), Some(&expected));
}

#[test]
fn fixed_aggregates_cross_link_in_both_directions() {
    const CCC_CALLER: &str = r#"
struct Mixed { long integer; double real; };
struct Big { long first; long second; long third; };

struct Mixed reference_mixed(struct Mixed value);
struct Big reference_big(struct Big value);

int ccc_entry(void) {
    struct Mixed mixed;
    struct Big big;
    struct Mixed mixed_result;
    struct Big big_result;
    mixed.integer = 17;
    mixed.real = 2.5;
    big.first = 3;
    big.second = 5;
    big.third = 7;
    mixed_result = reference_mixed(mixed);
    big_result = reference_big(big);
    if (mixed_result.integer != 19 || mixed_result.real != 5.0)
        return 11;
    if (big_result.first != 6 || big_result.second != 10 || big_result.third != 14)
        return 12;
    return 0;
}
"#;

    fn fixed_aggregate_classifier_shapes_cross_link_in_both_directions() {
        const CCC_SOURCE: &str = r#"
struct SsePair { double first; double second; };
struct Inner { long value; };
struct Nested { struct Inner inner; double real; };
union Overlay { long integer; double real; };
struct Bits { unsigned first : 4; unsigned second : 12; long tail; };
#pragma pack(push, 1)
struct Packed { char tag; long value; };
#pragma pack(pop)
struct Pair { long first; long second; };
struct Big { long first; long second; long third; };

struct SsePair reference_sse(struct SsePair);
struct Nested reference_nested(struct Nested);
union Overlay reference_union(union Overlay);
struct Bits reference_bits(struct Bits);
struct Packed reference_packed(struct Packed);
struct Pair reference_gp(long, long, long, long, long, struct Pair);
struct SsePair reference_sse_exhaustion(
    double, double, double, double, double, double, double, struct SsePair
);
struct Pair reference_indirect(struct Pair);
struct Big reference_alias(struct Big *);

int ccc_shapes_entry(void) {
    struct SsePair sse;
    struct Nested nested;
    union Overlay overlay;
    struct Bits bits;
    struct Packed packed;
    struct Pair pair;
    struct Big big;
    struct Pair (*indirect)(struct Pair) = reference_indirect;
    sse.first = 1.5; sse.second = 2.5;
    nested.inner.value = 3; nested.real = 4.0;
    overlay.integer = 7;
    bits.first = 2; bits.second = 100; bits.tail = 9;
    packed.tag = 4; packed.value = 20;
    pair.first = 1; pair.second = 2;
    big.first = 3; big.second = 4; big.third = 5;
    sse = reference_sse(sse);
    if (sse.first != 2.5 || sse.second != 4.5) return 61;
    nested = reference_nested(nested);
    if (nested.inner.value != 8 || nested.real != 10.0) return 62;
    overlay = reference_union(overlay);
    if (overlay.integer != 15) return 63;
    bits = reference_bits(bits);
    if (bits.first != 3 || bits.second != 102 || bits.tail != 12) return 64;
    packed = reference_packed(packed);
    if (packed.tag != 5 || packed.value != 22) return 65;
    pair.first = 1; pair.second = 2;
    pair = reference_gp(1, 2, 3, 4, 5, pair);
    if (pair.first != 16 || pair.second != 17) return 66;
    sse.first = 1.5; sse.second = 2.5;
    sse = reference_sse_exhaustion(1, 2, 3, 4, 5, 6, 7, sse);
    if (sse.first != 29.5 || sse.second != 30.5) return 67;
    pair.first = 1; pair.second = 2;
    pair = indirect(pair);
    if (pair.first != 31 || pair.second != 42) return 68;
    big = reference_alias(&big);
    if (big.first != 13 || big.second != 24 || big.third != 35) return 69;
    return 0;
}

struct SsePair ccc_sse(struct SsePair value) {
    value.first += 1.0; value.second += 2.0; return value;
}
struct Nested ccc_nested(struct Nested value) {
    value.inner.value += 5; value.real += 6.0; return value;
}
union Overlay ccc_union(union Overlay value) { value.integer += 8; return value; }
struct Bits ccc_bits(struct Bits value) {
    value.first += 1; value.second += 2; value.tail += 3; return value;
}
struct Packed ccc_packed(struct Packed value) {
    value.tag += 1; value.value += 2; return value;
}
struct Pair ccc_gp(long a, long b, long c, long d, long e, struct Pair value) {
    long sum = a + b + c + d + e;
    value.first += sum; value.second += sum; return value;
}
struct SsePair ccc_sse_exhaustion(
    double a, double b, double c, double d, double e, double f, double g,
    struct SsePair value
) {
    double sum = a + b + c + d + e + f + g;
    value.first += sum; value.second += sum; return value;
}
struct Pair ccc_indirect(struct Pair value) {
    value.first += 30; value.second += 40; return value;
}
struct Big ccc_alias(struct Big *visible) {
    struct Big result;
    result.first = visible->first + 10;
    result.second = visible->second + 20;
    result.third = visible->third + 30;
    return result;
}
"#;
        const REFERENCE_SOURCE: &str = r#"
struct SsePair { double first; double second; };
struct Inner { long value; };
struct Nested { struct Inner inner; double real; };
union Overlay { long integer; double real; };
struct Bits { unsigned first : 4; unsigned second : 12; long tail; };
#pragma pack(push, 1)
struct Packed { char tag; long value; };
#pragma pack(pop)
struct Pair { long first; long second; };
struct Big { long first; long second; long third; };

struct SsePair reference_sse(struct SsePair value) {
    value.first += 1.0; value.second += 2.0; return value;
}
struct Nested reference_nested(struct Nested value) {
    value.inner.value += 5; value.real += 6.0; return value;
}
union Overlay reference_union(union Overlay value) { value.integer += 8; return value; }
struct Bits reference_bits(struct Bits value) {
    value.first += 1; value.second += 2; value.tail += 3; return value;
}
struct Packed reference_packed(struct Packed value) {
    value.tag += 1; value.value += 2; return value;
}
struct Pair reference_gp(long a, long b, long c, long d, long e, struct Pair value) {
    long sum = a + b + c + d + e;
    value.first += sum; value.second += sum; return value;
}
struct SsePair reference_sse_exhaustion(
    double a, double b, double c, double d, double e, double f, double g,
    struct SsePair value
) {
    double sum = a + b + c + d + e + f + g;
    value.first += sum; value.second += sum; return value;
}
struct Pair reference_indirect(struct Pair value) {
    value.first += 30; value.second += 40; return value;
}
struct Big reference_alias(struct Big *visible) {
    struct Big result;
    result.first = visible->first + 10;
    result.second = visible->second + 20;
    result.third = visible->third + 30;
    return result;
}

int ccc_shapes_entry(void);
struct SsePair ccc_sse(struct SsePair);
struct Nested ccc_nested(struct Nested);
union Overlay ccc_union(union Overlay);
struct Bits ccc_bits(struct Bits);
struct Packed ccc_packed(struct Packed);
struct Pair ccc_gp(long, long, long, long, long, struct Pair);
struct SsePair ccc_sse_exhaustion(
    double, double, double, double, double, double, double, struct SsePair
);
struct Pair ccc_indirect(struct Pair);
struct Big ccc_alias(struct Big *);

int main(void) {
    struct SsePair sse;
    struct Nested nested;
    union Overlay overlay;
    struct Bits bits;
    struct Packed packed;
    struct Pair pair;
    struct Big big;
    struct Pair (*indirect)(struct Pair) = ccc_indirect;
    int status = ccc_shapes_entry();
    if (status != 0) return status;
    sse.first = 1.5; sse.second = 2.5;
    nested.inner.value = 3; nested.real = 4.0;
    overlay.integer = 7;
    bits.first = 2; bits.second = 100; bits.tail = 9;
    packed.tag = 4; packed.value = 20;
    pair.first = 1; pair.second = 2;
    big.first = 3; big.second = 4; big.third = 5;
    sse = ccc_sse(sse);
    if (sse.first != 2.5 || sse.second != 4.5) return 71;
    nested = ccc_nested(nested);
    if (nested.inner.value != 8 || nested.real != 10.0) return 72;
    overlay = ccc_union(overlay);
    if (overlay.integer != 15) return 73;
    bits = ccc_bits(bits);
    if (bits.first != 3 || bits.second != 102 || bits.tail != 12) return 74;
    packed = ccc_packed(packed);
    if (packed.tag != 5 || packed.value != 22) return 75;
    pair.first = 1; pair.second = 2;
    pair = ccc_gp(1, 2, 3, 4, 5, pair);
    if (pair.first != 16 || pair.second != 17) return 76;
    sse.first = 1.5; sse.second = 2.5;
    sse = ccc_sse_exhaustion(1, 2, 3, 4, 5, 6, 7, sse);
    if (sse.first != 29.5 || sse.second != 30.5) return 77;
    pair.first = 1; pair.second = 2;
    pair = indirect(pair);
    if (pair.first != 31 || pair.second != 42) return 78;
    big = ccc_alias(&big);
    if (big.first != 13 || big.second != 24 || big.third != 35) return 79;
    return 0;
}
"#;

        for (compiler, optimization) in reference_configurations() {
            let directory = test_directory("fixed-shapes", compiler, optimization);
            let ccc_source = write(&directory, "ccc-shapes.c", CCC_SOURCE);
            let reference_source = write(&directory, "reference-shapes.c", REFERENCE_SOURCE);
            let ccc_object = directory.join("ccc-shapes.o");
            let reference_object = directory.join("reference-shapes.o");
            compile_ccc(compiler, &ccc_source, &ccc_object);
            compile_reference(compiler, optimization, &reference_source, &reference_object);
            let output = link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
            fs::remove_dir_all(directory).unwrap();
        }
    }
    const REFERENCE_CALLEE: &str = r#"
struct Mixed { long integer; double real; };
struct Big { long first; long second; long third; };

struct Mixed reference_mixed(struct Mixed value) {
    value.integer += 2;
    value.real *= 2.0;
    return value;
}

struct Big reference_big(struct Big value) {
    value.first *= 2;
    value.second *= 2;
    value.third *= 2;
    return value;
}

int ccc_entry(void);
int main(void) { return ccc_entry(); }
"#;
    const CCC_CALLEE: &str = r#"
struct Pair { long first; long second; };
struct Mixed { long integer; double real; };

struct Pair ccc_pair(struct Pair value) {
    struct Pair result;
    result.first = value.first + 10;
    result.second = value.second + 20;
    return result;
}

struct Mixed ccc_mixed(struct Mixed value) {
    struct Mixed result;
    result.integer = value.integer + 3;
    result.real = value.real + 4.0;
    return result;
}
"#;
    const REFERENCE_CALLER: &str = r#"
struct Pair { long first; long second; };
struct Mixed { long integer; double real; };

struct Pair ccc_pair(struct Pair);
struct Mixed ccc_mixed(struct Mixed);

int main(void) {
    struct Pair pair = { 1, 2 };
    struct Mixed mixed = { 5, 6.0 };
    pair = ccc_pair(pair);
    mixed = ccc_mixed(mixed);
    if (pair.first != 11 || pair.second != 22)
        return 21;
    if (mixed.integer != 8 || mixed.real != 10.0)
        return 22;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("fixed", compiler, optimization);
        let ccc_caller = write(&directory, "ccc-caller.c", CCC_CALLER);
        let reference_callee = write(&directory, "reference-callee.c", REFERENCE_CALLEE);
        let ccc_caller_o = directory.join("ccc-caller.o");
        let reference_callee_o = directory.join("reference-callee.o");
        compile_ccc(compiler, &ccc_caller, &ccc_caller_o);
        compile_reference(
            compiler,
            optimization,
            &reference_callee,
            &reference_callee_o,
        );
        link_and_run(compiler, &directory, &[&ccc_caller_o, &reference_callee_o]);

        let ccc_callee = write(&directory, "ccc-callee.c", CCC_CALLEE);
        let reference_caller = write(&directory, "reference-caller.c", REFERENCE_CALLER);
        let ccc_callee_o = directory.join("ccc-callee.o");
        let reference_caller_o = directory.join("reference-caller.o");
        compile_ccc(compiler, &ccc_callee, &ccc_callee_o);
        compile_reference(
            compiler,
            optimization,
            &reference_caller,
            &reference_caller_o,
        );
        link_and_run(compiler, &directory, &[&ccc_callee_o, &reference_caller_o]);
        fs::remove_dir_all(directory).unwrap();
    }
    fixed_aggregate_classifier_shapes_cross_link_in_both_directions();
}

#[test]
fn variadic_calls_and_definitions_cross_link_in_both_directions() {
    const CCC_CALLER: &str = r#"
int reference_variadic(int marker, ...);

int ccc_entry(void) {
    return reference_variadic(
        9,
        1, 2, 3, 4, 5, 6, 7, 8,
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0
    );
}
"#;
    const REFERENCE_CALLEE: &str = r#"
#include <stdarg.h>

int reference_variadic(int marker, ...) {
    va_list ap;
    int integers = 0;
    int reals = 0;
    int index;
    va_start(ap, marker);
    for (index = 0; index < 8; ++index)
        integers += va_arg(ap, int);
    for (index = 0; index < 10; ++index)
        reals += (int)va_arg(ap, double);
    va_end(ap);
    return marker + integers + reals == 100 ? 0 : 31;
}

int ccc_entry(void);
int main(void) { return ccc_entry(); }
"#;
    const CCC_CALLEE: &str = r#"
#include <stdarg.h>

int ccc_variadic(int marker, ...) {
    va_list ap;
    int integers = 0;
    int reals = 0;
    int index;
    va_start(ap, marker);
    for (index = 0; index < 8; ++index)
        integers += va_arg(ap, int);
    for (index = 0; index < 10; ++index)
        reals += (int)va_arg(ap, double);
    va_end(ap);
    return marker + integers + reals;
}
"#;
    const REFERENCE_CALLER: &str = r#"
int ccc_variadic(int marker, ...);

int main(void) {
    int result = ccc_variadic(
        9,
        1, 2, 3, 4, 5, 6, 7, 8,
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0
    );
    return result == 100 ? 0 : 32;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("variadic", compiler, optimization);
        let ccc_caller = write(&directory, "ccc-caller.c", CCC_CALLER);
        let reference_callee = write(&directory, "reference-callee.c", REFERENCE_CALLEE);
        let ccc_caller_o = directory.join("ccc-caller.o");
        let reference_callee_o = directory.join("reference-callee.o");
        compile_ccc(compiler, &ccc_caller, &ccc_caller_o);
        compile_reference(
            compiler,
            optimization,
            &reference_callee,
            &reference_callee_o,
        );
        link_and_run(compiler, &directory, &[&ccc_caller_o, &reference_callee_o]);

        let ccc_callee = write(&directory, "ccc-callee.c", CCC_CALLEE);
        let reference_caller = write(&directory, "reference-caller.c", REFERENCE_CALLER);
        let ccc_callee_o = directory.join("ccc-callee.o");
        let reference_caller_o = directory.join("reference-caller.o");
        compile_ccc(compiler, &ccc_callee, &ccc_callee_o);
        compile_reference(
            compiler,
            optimization,
            &reference_caller,
            &reference_caller_o,
        );
        link_and_run(compiler, &directory, &[&ccc_callee_o, &reference_caller_o]);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn external_va_lists_round_trip_through_libc_and_ccc() {
    const CCC_SOURCE: &str = r#"
#include <stdarg.h>

typedef unsigned long size_t;
typedef struct _IO_FILE FILE;

int vsnprintf(char *buffer, size_t size, const char *format, va_list ap);
int vfprintf(FILE *stream, const char *format, va_list ap);

int ccc_to_vsnprintf(char *buffer, size_t size, const char *format, ...) {
    va_list ap;
    int result;
    va_start(ap, format);
    result = vsnprintf(buffer, size, format, ap);
    va_end(ap);
    return result;
}

int ccc_to_vfprintf(FILE *stream, const char *format, ...) {
    va_list ap;
    int result;
    va_start(ap, format);
    result = vfprintf(stream, format, ap);
    va_end(ap);
    return result;
}

int ccc_consume(va_list incoming) {
    va_list first;
    va_list second;
    int first_sum;
    int second_sum;
    va_copy(first, incoming);
    va_copy(second, incoming);
    first_sum = va_arg(first, int) + (int)va_arg(first, double) + (int)va_arg(first, long long);
    second_sum = va_arg(second, int) + (int)va_arg(second, double) + (int)va_arg(second, long long);
    va_end(first);
    va_end(second);
    return first_sum + second_sum;
}
"#;
    const REFERENCE_SOURCE: &str = r#"
#include <stdarg.h>
#include <stdio.h>
#include <string.h>

int ccc_to_vsnprintf(char *, size_t, const char *, ...);
int ccc_to_vfprintf(FILE *, const char *, ...);
int ccc_consume(va_list);

static int reference_to_ccc(int marker, ...) {
    va_list ap;
    int result;
    va_start(ap, marker);
    result = ccc_consume(ap);
    va_end(ap);
    return result;
}

int main(void) {
    char buffer[128];
    char file_buffer[128];
    FILE *stream;
    size_t count;
    int result = ccc_to_vsnprintf(buffer, sizeof(buffer), "%d %.1f %s", 7, 8.0, "nine");
    if (result != 10 || strcmp(buffer, "7 8.0 nine") != 0)
        return 41;
    stream = tmpfile();
    if (stream == NULL)
        return 42;
    result = ccc_to_vfprintf(stream, "%d %.1f %s", 7, 8.0, "nine");
    if (result != 10)
        return 43;
    rewind(stream);
    count = fread(file_buffer, 1, sizeof(file_buffer) - 1, stream);
    file_buffer[count] = '\0';
    fclose(stream);
    if (strcmp(file_buffer, "7 8.0 nine") != 0)
        return 44;
    if (reference_to_ccc(0, 7, 8.0, 9LL) != 48)
        return 45;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("va-list", compiler, optimization);
        let ccc_source = write(&directory, "ccc-source.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-source.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-source.o");
        let reference_object = directory.join("reference-source.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn default_argument_promotions_cross_ellipsis_in_both_directions() {
    const CCC_SOURCE: &str = r#"
#include <stdarg.h>

int reference_promotions(int marker, ...);

int ccc_promotions(int marker, ...) {
    va_list ap;
    double floating;
    int character;
    int short_value;
    va_start(ap, marker);
    floating = va_arg(ap, double);
    character = va_arg(ap, int);
    short_value = va_arg(ap, int);
    va_end(ap);
    if (floating != 1.25)
        return 1;
    if (character != -7)
        return 2;
    if (short_value != 60000)
        return 3;
    return 0;
}

int ccc_calls_promotions(void) {
    float floating = 1.25f;
    signed char character = -7;
    unsigned short short_value = 60000;
    return reference_promotions(0, floating, character, short_value);
}
"#;
    const REFERENCE_SOURCE: &str = r#"
#include <stdarg.h>

int ccc_promotions(int marker, ...);
int ccc_calls_promotions(void);

int reference_promotions(int marker, ...) {
    va_list ap;
    double floating;
    int character;
    int short_value;
    va_start(ap, marker);
    floating = va_arg(ap, double);
    character = va_arg(ap, int);
    short_value = va_arg(ap, int);
    va_end(ap);
    if (floating != 1.25)
        return 4;
    if (character != -7)
        return 5;
    if (short_value != 60000)
        return 6;
    return 0;
}

int main(void) {
    float floating = 1.25f;
    signed char character = -7;
    unsigned short short_value = 60000;
    int status = ccc_calls_promotions();
    if (status != 0)
        return 111 + status;
    status = ccc_promotions(0, floating, character, short_value);
    if (status != 0)
        return 121 + status;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("default-promotions", compiler, optimization);
        let ccc_source = write(&directory, "ccc-promotions.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-promotions.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-promotions.o");
        let reference_object = directory.join("reference-promotions.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        let output = link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn variadic_call_bridge_sets_the_vector_register_count() {
    const CCC_SOURCE: &str = r#"
int reference_al(int marker, ...);

int ccc_entry(void) {
    int (*indirect)(int, ...) = reference_al;
    if (reference_al(0) != 0)
        return 51;
    if (reference_al(0, 1.0) != 1)
        return 52;
    if (reference_al(0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0) != 8)
        return 53;
    if (indirect(0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0) != 8)
        return 54;
    return 0;
}
"#;
    const REFERENCE_SOURCE: &str = r#"
__attribute__((noinline)) int reference_al(int marker, ...) {
    unsigned int count;
    (void)marker;
    __asm__ volatile ("movzbl %%al, %0" : "=r"(count));
    return (int)count;
}

int ccc_entry(void);
int main(void) { return ccc_entry(); }
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("al", compiler, optimization);
        let ccc_source = write(&directory, "ccc-source.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-source.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-source.o");
        let reference_object = directory.join("reference-source.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        let output = link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn variadic_aggregate_state_and_reentrancy_cross_link_in_both_directions() {
    const CCC_SOURCE: &str = r#"
#include <stdarg.h>

struct GpPair { long first; long second; };
struct Mixed { long integer; double real; };
struct Big { long first; long second; long third; };

struct Big reference_collect(int, ...);
long reference_seven(long, long, long, long, long, long, long, ...);
long reference_mixed_prefix(
    int, double, double, double, double, double, double, double, ...
);
int reference_reenter(int (*callback)(int, ...), ...);

struct Big ccc_collect(int marker, ...) {
    va_list ap;
    int *pointer;
    struct GpPair gp;
    struct Mixed mixed;
    struct Big big;
    struct Big result;
    va_start(ap, marker);
    pointer = va_arg(ap, int *);
    gp = va_arg(ap, struct GpPair);
    mixed = va_arg(ap, struct Mixed);
    big = va_arg(ap, struct Big);
    va_end(ap);
    result.first = *pointer + gp.first + mixed.integer + big.first + marker;
    result.second = gp.second + (long)mixed.real + big.second;
    result.third = big.third + 1;
    return result;
}

long ccc_seven(
    long a, long b, long c, long d, long e, long f, long g, ...
) {
    va_list ap;
    long first;
    long second;
    va_start(ap, g);
    first = va_arg(ap, long);
    va_end(ap);
    va_start(ap, g);
    second = va_arg(ap, long);
    va_end(ap);
    return a + b + c + d + e + f + g + first + second;
}

long ccc_mixed_prefix(
    int marker,
    double a, double b, double c, double d, double e, double f, double g,
    ...
) {
    va_list ap;
    double register_value;
    double overflow_value;
    va_start(ap, g);
    register_value = va_arg(ap, double);
    overflow_value = va_arg(ap, double);
    va_end(ap);
    return (long)(marker + a + b + c + d + e + f + g
                  + register_value + overflow_value);
}

int ccc_recursive(int depth, ...) {
    va_list ap;
    int value;
    va_start(ap, depth);
    value = va_arg(ap, int);
    va_end(ap);
    if (depth == 0)
        return value;
    return value + ccc_recursive(depth - 1, value + 1);
}

static int ccc_static_sum(int marker, ...) {
    va_list ap;
    int value;
    va_start(ap, marker);
    value = va_arg(ap, int);
    va_end(ap);
    return marker + value;
}

int ccc_static_entry(void) {
    int (*indirect)(int, ...) = ccc_static_sum;
    return ccc_static_sum(1, 2) + indirect(3, 4);
}

int ccc_matrix_entry(void) {
    int pointed = 5;
    struct GpPair gp;
    struct Mixed mixed;
    struct Big big;
    struct Big result;
    gp.first = 7; gp.second = 11;
    mixed.integer = 13; mixed.real = 17.0;
    big.first = 19; big.second = 23; big.third = 29;
    result = reference_collect(3, &pointed, gp, mixed, big);
    if (result.first != 47 || result.second != 51 || result.third != 30)
        return 81;
    if (reference_seven(1, 2, 3, 4, 5, 6, 7, 11L) != 50)
        return 82;
    if (reference_mixed_prefix(2, 1, 2, 3, 4, 5, 6, 7, 8.0, 9.0) != 47)
        return 83;
    if (reference_reenter(ccc_recursive, 2, 10) != 33)
        return 84;
    if (ccc_static_entry() != 10)
        return 85;
    return 0;
}
"#;
    const REFERENCE_SOURCE: &str = r#"
#include <stdarg.h>

struct GpPair { long first; long second; };
struct Mixed { long integer; double real; };
struct Big { long first; long second; long third; };

struct Big reference_collect(int marker, ...) {
    va_list ap;
    int *pointer;
    struct GpPair gp;
    struct Mixed mixed;
    struct Big big;
    struct Big result;
    va_start(ap, marker);
    pointer = va_arg(ap, int *);
    gp = va_arg(ap, struct GpPair);
    mixed = va_arg(ap, struct Mixed);
    big = va_arg(ap, struct Big);
    va_end(ap);
    result.first = *pointer + gp.first + mixed.integer + big.first + marker;
    result.second = gp.second + (long)mixed.real + big.second;
    result.third = big.third + 1;
    return result;
}

long reference_seven(
    long a, long b, long c, long d, long e, long f, long g, ...
) {
    va_list ap;
    long first;
    long second;
    va_start(ap, g);
    first = va_arg(ap, long);
    va_end(ap);
    va_start(ap, g);
    second = va_arg(ap, long);
    va_end(ap);
    return a + b + c + d + e + f + g + first + second;
}

long reference_mixed_prefix(
    int marker,
    double a, double b, double c, double d, double e, double f, double g,
    ...
) {
    va_list ap;
    double register_value;
    double overflow_value;
    va_start(ap, g);
    register_value = va_arg(ap, double);
    overflow_value = va_arg(ap, double);
    va_end(ap);
    return (long)(marker + a + b + c + d + e + f + g
                  + register_value + overflow_value);
}

int reference_reenter(int (*callback)(int, ...), ...) {
    va_list ap;
    int depth;
    int value;
    va_start(ap, callback);
    depth = va_arg(ap, int);
    value = va_arg(ap, int);
    va_end(ap);
    return callback(depth, value);
}

struct Big ccc_collect(int, ...);
long ccc_seven(long, long, long, long, long, long, long, ...);
long ccc_mixed_prefix(
    int, double, double, double, double, double, double, double, ...
);
int ccc_recursive(int, ...);
int ccc_static_entry(void);
int ccc_matrix_entry(void);

int main(void) {
    int pointed = 6;
    struct GpPair gp;
    struct Mixed mixed;
    struct Big big;
    struct Big result;
    int status = ccc_matrix_entry();
    if (status != 0)
        return status;
    gp.first = 8; gp.second = 12;
    mixed.integer = 14; mixed.real = 18.0;
    big.first = 20; big.second = 24; big.third = 30;
    result = ccc_collect(4, &pointed, gp, mixed, big);
    if (result.first != 52 || result.second != 54 || result.third != 31)
        return 85;
    if (ccc_seven(1, 2, 3, 4, 5, 6, 7, 12L) != 52)
        return 86;
    if (ccc_mixed_prefix(3, 1, 2, 3, 4, 5, 6, 7, 10.0, 11.0) != 52)
        return 87;
    if (ccc_recursive(2, 10) != 33)
        return 88;
    if (ccc_static_entry() != 10)
        return 89;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("variadic-matrix", compiler, optimization);
        let ccc_source = write(&directory, "ccc-matrix.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-matrix.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-matrix.o");
        let reference_object = directory.join("reference-matrix.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        let output = link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn packaged_bridges_have_deterministic_symbols_relocations_and_metadata() {
    const SOURCE: &str = r#"
#include <stdarg.h>

int reference_probe(int marker, ...);

int public_variadic(int marker, ...) {
    va_list ap;
    int value;
    va_start(ap, marker);
    value = va_arg(ap, int);
    va_end(ap);
    return marker + value;
}

static int private_variadic(int marker, ...) {
    va_list ap;
    int value;
    va_start(ap, marker);
    value = va_arg(ap, int);
    va_end(ap);
    return marker + value;
}

int private_entry(void) {
    int (*indirect)(int, ...) = private_variadic;
    return private_variadic(1, 2) + indirect(3, 4) + public_variadic(5, 6);
}

int calls_reference(void) {
    int (*indirect)(int, ...) = reference_probe;
    return reference_probe(
        0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0
    ) + indirect(0, 11);
}
"#;

    let directory = test_directory("object-contract", "gcc", "-O0");
    let first_source = write(&directory, "first.c", SOURCE);
    let second_source = write(&directory, "second.c", SOURCE);
    let first_object = directory.join("first.o");
    let second_object = directory.join("second.o");

    let dump = run(Command::new(env!("CARGO_BIN_EXE_ccc"))
        .env("CCC_CC", "gcc")
        .arg("--dump-abi")
        .arg(&first_source));
    let dump = String::from_utf8(dump.stdout).unwrap();
    assert!(dump.lines().any(|line| {
        line.contains("transport=bridge kind=variadic-call") && line.contains(" al=8 ")
    }));

    compile_ccc("gcc", &first_source, &first_object);
    compile_ccc("gcc", &second_source, &second_object);

    let inspect = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let mut generated = Vec::new();
        let mut defined = BTreeSet::new();
        let mut relocation_counts = BTreeMap::<String, usize>::new();
        for symbol in object.symbols() {
            let Ok(name) = symbol.name() else { continue };
            if name.is_empty() {
                continue;
            }
            if !symbol.is_undefined() {
                defined.insert(name.to_owned());
            }
            if bridge_generated_symbol(name) {
                assert!(
                    !symbol.is_undefined(),
                    "packaged object retains unresolved generated symbol {name}"
                );
                generated.push((name.to_owned(), symbol.scope()));
            }
        }
        generated.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for section in object.sections() {
            for (_, relocation) in section.relocations() {
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    continue;
                };
                let symbol = object.symbol_by_index(index).unwrap();
                let Ok(name) = symbol.name() else { continue };
                if bridge_generated_symbol(name) {
                    assert!(
                        defined.contains(name),
                        "relocation targets generated symbol outside the packaged object: {name}"
                    );
                }
                *relocation_counts.entry(name.to_owned()).or_default() += 1;
            }
        }
        let helper = generated
            .iter()
            .filter(|(name, _)| name.starts_with("__ccc_call_helper_"))
            .collect::<Vec<_>>();
        let bodies = generated
            .iter()
            .filter(|(name, _)| name.starts_with("__ccc_variadic_body_"))
            .collect::<Vec<_>>();
        assert_eq!(helper.len(), 1, "one helper must serve every call shape");
        assert_eq!(bodies.len(), 2, "each variadic definition needs one body");
        assert!(
            generated
                .iter()
                .all(|(_, scope)| *scope == SymbolScope::Compilation)
        );
        assert!(relocation_counts.get(&helper[0].0).copied().unwrap_or(0) >= 5);
        for (body, _) in &bodies {
            assert_eq!(
                relocation_counts.get(body).copied(),
                Some(1),
                "each entry must call its exact hidden body once"
            );
        }

        let public = object
            .symbols()
            .find(|symbol| symbol.name() == Ok("public_variadic"))
            .unwrap();
        let private = object
            .symbols()
            .find(|symbol| symbol.name() == Ok("private_variadic"))
            .unwrap();
        assert_eq!(public.scope(), SymbolScope::Dynamic);
        assert_eq!(private.scope(), SymbolScope::Compilation);
        assert!(
            relocation_counts
                .get("public_variadic")
                .copied()
                .unwrap_or(0)
                >= 1
        );
        assert!(
            relocation_counts
                .get("private_variadic")
                .copied()
                .unwrap_or(0)
                >= 2
        );
        assert!(object.section_by_name(".eh_frame").is_some());
        assert!(object.section_by_name(".debug_line").is_none());
        let file_symbols = object
            .symbols()
            .filter(|symbol| symbol.kind() == object::SymbolKind::File)
            .map(|symbol| symbol.name().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            file_symbols,
            ["ccc"],
            "bridge packaging must preserve only Cranelift's compiler-owned ELF file symbol"
        );
        let stack_note = object.section_by_name(".note.GNU-stack").unwrap();
        assert!(!matches!(
            stack_note.flags(),
            SectionFlags::Elf { sh_flags }
                if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
        ));
        generated
    };

    let first_symbols = inspect(&first_object);
    let second_symbols = inspect(&second_object);
    assert_eq!(first_symbols, second_symbols);

    let helper = first_symbols
        .iter()
        .find(|(name, _)| name.starts_with("__ccc_call_helper_"))
        .unwrap()
        .0
        .clone();
    let disassembly = run(Command::new("objdump").args(["-drwC"]).arg(&first_object));
    let disassembly = String::from_utf8(disassembly.stdout).unwrap();
    let helper_label = format!("<{helper}>:");
    let helper_start = disassembly.find(&helper_label).unwrap();
    let helper_tail = &disassembly[helper_start..];
    let helper_end = helper_tail.find("\n\n").unwrap_or(helper_tail.len());
    let helper_body = &helper_tail[..helper_end];
    let vector_count = helper_body
        .lines()
        .position(|line| line.split_ascii_whitespace().any(|field| field == "movzbl"))
        .unwrap();
    let indirect_call = helper_body
        .lines()
        .position(|line| {
            line.split_ascii_whitespace()
                .any(|field| matches!(field, "call" | "callq"))
        })
        .unwrap();
    assert!(helper_body.contains("0x1a(%r12)"));
    assert!(
        vector_count < indirect_call,
        "%al must be populated last before the call"
    );
    assert!(disassembly.matches(&helper).count() >= 2);

    let frames = run(Command::new("readelf")
        .args(["--debug-dump=frames", "--wide"])
        .arg(&first_object));
    let frames = String::from_utf8(frames.stdout).unwrap();
    let fde_ranges = frames
        .lines()
        .filter_map(|line| {
            let (_, pc) = line.split_once(" pc=")?;
            let range = pc.split_whitespace().next()?;
            let (start, end) = range.split_once("..")?;
            Some((
                u64::from_str_radix(start.trim_start_matches("0x"), 16).ok()?,
                u64::from_str_radix(end.trim_start_matches("0x"), 16).ok()?,
            ))
        })
        .collect::<Vec<_>>();
    let bytes = fs::read(&first_object).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    for name in [helper.as_str(), "public_variadic", "private_variadic"] {
        let symbol = object
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .unwrap();
        let start = symbol.address();
        let end = start + symbol.size();
        assert!(
            fde_ranges
                .iter()
                .any(|(fde_start, fde_end)| *fde_start == start && *fde_end >= end),
            "generated function {name} at {start:#x}..{end:#x} has no covering FDE:\n{frames}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn packaged_bridges_are_cwd_independent_and_path_free() {
    const SOURCE: &str = r#"
#include <stdarg.h>

int sum(int count, ...) {
    va_list ap;
    int total = 0;
    va_start(ap, count);
    while (count > 0) {
        total += va_arg(ap, int);
        count = count - 1;
    }
    va_end(ap);
    return total;
}

int exercise(void) {
    return sum(2, 3, 4);
}
"#;

    for compiler in reference_compilers() {
        let directory = test_directory("cwd-determinism", compiler, "-O0");
        let source_directory = directory.join("shared-source");
        let first_directory = directory.join("first-working-directory");
        let second_directory = directory.join("second-working-directory");
        for path in [&source_directory, &first_directory, &second_directory] {
            fs::create_dir(path).unwrap();
        }
        let source = write(&source_directory, "same-input.c", SOURCE);
        let first_object = first_directory.join("artifact.o");
        let second_object = second_directory.join("artifact.o");

        let bridge =
            ccc_link::bridge::render_generic_call_helper("__ccc_call_helper_cwd_reproducibility")
                .unwrap();
        let first_bridge_source = write(&first_directory, "same-bridge.s", bridge.source());
        let second_bridge_source = write(&second_directory, "same-bridge.s", bridge.source());
        let first_bridge_object = first_directory.join("same-bridge.o");
        let second_bridge_object = second_directory.join("same-bridge.o");
        for (working_directory, source, object) in [
            (&first_directory, &first_bridge_source, &first_bridge_object),
            (
                &second_directory,
                &second_bridge_source,
                &second_bridge_object,
            ),
        ] {
            run(Command::new(compiler)
                .current_dir(working_directory)
                .args(["-x", "assembler", "-c"])
                .arg(source.file_name().unwrap())
                .arg("-o")
                .arg(object.file_name().unwrap()));
        }
        let first_bridge = fs::read(&first_bridge_object).unwrap();
        let second_bridge = fs::read(&second_bridge_object).unwrap();
        assert_eq!(
            first_bridge, second_bridge,
            "{compiler} bridge assembly depended on the assembler working directory"
        );
        let bridge_object = object::File::parse(first_bridge.as_slice()).unwrap();
        assert!(bridge_object.section_by_name(".eh_frame").is_some());
        assert!(bridge_object.section_by_name(".debug_line").is_none());
        assert!(
            bridge_object
                .symbols()
                .all(|symbol| symbol.kind() != object::SymbolKind::File),
            "{compiler} produced an unwanted ELF file symbol for bridge assembly"
        );
        let bridge_stack_note = bridge_object.section_by_name(".note.GNU-stack").unwrap();
        assert!(!matches!(
            bridge_stack_note.flags(),
            SectionFlags::Elf { sh_flags }
                if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
        ));

        for (working_directory, object) in [
            (&first_directory, &first_object),
            (&second_directory, &second_object),
        ] {
            run(Command::new(env!("CARGO_BIN_EXE_ccc"))
                .current_dir(working_directory)
                .env("CCC_CC", compiler)
                .arg("-c")
                .arg(&source)
                .arg("-o")
                .arg(object));
        }

        let first = fs::read(&first_object).unwrap();
        let second = fs::read(&second_object).unwrap();
        assert_eq!(
            first, second,
            "{compiler} bridge packaging depended on the assembler working directory"
        );
        for path in [&first_directory, &second_directory] {
            let encoded = path.as_os_str().as_encoded_bytes();
            assert!(
                !first.windows(encoded.len()).any(|window| window == encoded),
                "packaged object leaks build path {}",
                path.display()
            );
        }

        let object = object::File::parse(first.as_slice()).unwrap();
        assert!(object.section_by_name(".eh_frame").is_some());
        assert!(object.section_by_name(".debug_line").is_none());
        let stack_note = object.section_by_name(".note.GNU-stack").unwrap();
        assert!(!matches!(
            stack_note.flags(),
            SectionFlags::Elf { sh_flags }
                if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn libgcc_backtrace_unwinds_across_both_bridge_kinds() {
    const CCC_SOURCE: &str = r#"
int reference_fixed_unwind_depth(void);
int reference_variadic_unwind_depth(int marker, ...);

int ccc_variadic_unwind_depth(int marker, ...) {
    return marker + reference_fixed_unwind_depth();
}

int ccc_call_helper_unwind_depth(void) {
    return reference_variadic_unwind_depth(0, 1.0);
}
"#;
    const REFERENCE_SOURCE: &str = r#"
#include <stdlib.h>
#include <unwind.h>

struct TraceState {
    int nonzero_frames;
    _Unwind_Ptr expected_region;
    int saw_expected_region;
};

static _Unwind_Ptr entry_bridge_region;
static _Unwind_Ptr call_helper_region;

static _Unwind_Reason_Code count_frame(
    struct _Unwind_Context *context,
    void *argument
) {
    struct TraceState *state = argument;
    if (_Unwind_GetIP(context) != 0)
        ++state->nonzero_frames;
    if (_Unwind_GetRegionStart(context) == state->expected_region)
        state->saw_expected_region = 1;
    return _URC_NO_REASON;
}

__attribute__((noinline)) static int capture_unwind_depth(_Unwind_Ptr expected_region) {
    struct TraceState state = { 0, expected_region, 0 };
    (void)_Unwind_Backtrace(count_frame, &state);
    return state.saw_expected_region ? state.nonzero_frames : -state.nonzero_frames;
}

__attribute__((noinline)) int reference_fixed_unwind_depth(void) {
    volatile int depth = capture_unwind_depth(entry_bridge_region);
    return depth;
}

__attribute__((noinline)) int reference_variadic_unwind_depth(int marker, ...) {
    volatile int depth;
    (void)marker;
    depth = capture_unwind_depth(call_helper_region);
    return depth;
}

int ccc_variadic_unwind_depth(int marker, ...);
int ccc_call_helper_unwind_depth(void);

static _Unwind_Ptr parse_region(const char *text) {
    char *end;
    unsigned long long value = strtoull(text, &end, 16);
    if (*text == '\0' || *end != '\0')
        return 0;
    return (_Unwind_Ptr)value;
}

int main(int argc, char **argv) {
    if (argc != 3)
        return 130;
    entry_bridge_region = parse_region(argv[1]);
    call_helper_region = parse_region(argv[2]);
    if (entry_bridge_region == 0 || call_helper_region == 0)
        return 130;
    int entry_depth = ccc_variadic_unwind_depth(0, 1);
    int helper_depth = ccc_call_helper_unwind_depth();
    if (entry_depth <= 0)
        return 131;
    if (helper_depth <= 0)
        return 132;
    if (entry_depth < 6 || helper_depth < 6)
        return 133;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("libgcc-unwind", compiler, optimization);
        let ccc_source = write(&directory, "ccc-unwind.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-unwind.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-unwind.o");
        let reference_object = directory.join("reference-unwind.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        let executable = directory.join("program");
        run(Command::new(compiler)
            .arg("-no-pie")
            .arg(&ccc_object)
            .arg(&reference_object)
            .arg("-o")
            .arg(&executable));

        let bytes = fs::read(&executable).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let entry_address = object
            .symbols()
            .find(|symbol| symbol.name() == Ok("ccc_variadic_unwind_depth"))
            .map(|symbol| symbol.address())
            .expect("public variadic entry symbol is present");
        let helper_addresses = object
            .symbols()
            .filter_map(|symbol| {
                symbol
                    .name()
                    .ok()
                    .filter(|name| name.starts_with("__ccc_call_helper_"))
                    .map(|_| symbol.address())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            helper_addresses.len(),
            1,
            "expected exactly one call helper"
        );
        let output = run(Command::new(&executable)
            .env("LC_ALL", "C")
            .arg(format!("{entry_address:x}"))
            .arg(format!("{:x}", helper_addresses[0])));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn debugger_unwinds_across_both_bridge_kinds() {
    const CCC_SOURCE: &str = r#"
#include <stdarg.h>

int reference_bridge(int marker, ...);

int public_entry(int marker, ...) {
    va_list ap;
    int value;
    va_start(ap, marker);
    value = va_arg(ap, int);
    va_end(ap);
    return marker + value;
}

int call_bridge(void) {
    return reference_bridge(7, 8);
}
"#;
    const REFERENCE_SOURCE: &str = r#"
#include <stdarg.h>

int public_entry(int, ...);
int call_bridge(void);

int reference_bridge(int marker, ...) {
    va_list ap;
    int value;
    va_start(ap, marker);
    value = va_arg(ap, int);
    va_end(ap);
    return marker + value;
}

int main(void) {
    if (public_entry(1, 2) != 3)
        return 91;
    if (call_bridge() != 15)
        return 92;
    return 0;
}
"#;

    let directory = test_directory("debugger", "gcc", "-O0");
    let ccc_source = write(&directory, "ccc-debugger.c", CCC_SOURCE);
    let reference_source = write(&directory, "reference-debugger.c", REFERENCE_SOURCE);
    let ccc_object = directory.join("ccc-debugger.o");
    let reference_object = directory.join("reference-debugger.o");
    compile_ccc("gcc", &ccc_source, &ccc_object);
    compile_reference("gcc", "-O0", &reference_source, &reference_object);
    link_and_run("gcc", &directory, &[&ccc_object, &reference_object]);
    let executable = directory.join("program");

    let debug = |break_command: &str| {
        let output = run(Command::new("timeout")
            .args([
                "--signal=KILL",
                "20s",
                "gdb",
                "--batch",
                "--quiet",
                "-ex",
                "set pagination off",
                "-ex",
                "set confirm off",
                "-ex",
                break_command,
                "-ex",
                "run",
                "-ex",
                "bt",
                "-ex",
                "continue",
            ])
            .arg(&executable));
        let mut text = String::from_utf8(output.stdout).unwrap();
        text.push_str(&String::from_utf8(output.stderr).unwrap());
        text
    };

    let entry = debug("break public_entry");
    assert!(entry.contains("public_entry"), "{entry}");
    assert!(entry.contains("main"), "{entry}");

    let call = debug("rbreak ^__ccc_call_helper_");
    assert!(call.contains("__ccc_call_helper_"), "{call}");
    assert!(call.contains("call_bridge"), "{call}");
    assert!(call.contains("main"), "{call}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn scalar_boundaries_cross_link_without_planner_regressions() {
    const CCC_SOURCE: &str = r#"
enum Choice { ChoiceZero, ChoiceOne, ChoiceTwo };

_Bool reference_boolean(_Bool);
signed char reference_signed_char(signed char);
unsigned char reference_unsigned_char(unsigned char);
short reference_short(short);
unsigned short reference_unsigned_short(unsigned short);
int reference_int(int);
long reference_long(long);
long long reference_long_long(long long);
enum Choice reference_choice(enum Choice);
int *reference_pointer(int *);
float reference_float(float);
double reference_double(double);

_Bool ccc_boolean(_Bool value) { return !value; }
signed char ccc_signed_char(signed char value) { return value + 1; }
unsigned char ccc_unsigned_char(unsigned char value) { return value + 1; }
short ccc_short(short value) { return value + 2; }
unsigned short ccc_unsigned_short(unsigned short value) { return value + 3; }
int ccc_int(int value) { return value + 4; }
long ccc_long(long value) { return value + 5; }
long long ccc_long_long(long long value) { return value + 6; }
enum Choice ccc_choice(enum Choice value) {
    if (value == ChoiceZero) return ChoiceTwo;
    return ChoiceZero;
}
int *ccc_pointer(int *value) { *value += 7; return value; }
float ccc_float(float value) { return value + 1.25f; }
double ccc_double(double value) { return value + 2.5; }

int ccc_scalar_entry(void) {
    signed char (*signed_char_call)(signed char) = reference_signed_char;
    unsigned short (*unsigned_short_call)(unsigned short) = reference_unsigned_short;
    long (*long_call)(long) = reference_long;
    enum Choice (*choice_call)(enum Choice) = reference_choice;
    double (*double_call)(double) = reference_double;
    int pointed = 9;
    if (reference_boolean(0) != 1) return 101;
    if (signed_char_call(-5) != -4) return 102;
    if (reference_unsigned_char(250) != 251) return 103;
    if (reference_short(-300) != -298) return 104;
    if (unsigned_short_call(60000) != 60003) return 105;
    if (reference_int(-1000) != -996) return 106;
    if (long_call(100000L) != 100005L) return 107;
    if (reference_long_long(10000000000LL) != 10000000006LL) return 108;
    if (choice_call(ChoiceZero) != ChoiceTwo) return 109;
    if (reference_pointer(&pointed) != &pointed || pointed != 16) return 110;
    if (reference_float(1.5f) != 2.75f) return 111;
    if (double_call(3.5) != 6.0) return 112;
    return 0;
}
"#;
    const REFERENCE_SOURCE: &str = r#"
enum Choice { ChoiceZero, ChoiceOne, ChoiceTwo };

_Bool reference_boolean(_Bool value) { return !value; }
signed char reference_signed_char(signed char value) { return value + 1; }
unsigned char reference_unsigned_char(unsigned char value) { return value + 1; }
short reference_short(short value) { return value + 2; }
unsigned short reference_unsigned_short(unsigned short value) { return value + 3; }
int reference_int(int value) { return value + 4; }
long reference_long(long value) { return value + 5; }
long long reference_long_long(long long value) { return value + 6; }
enum Choice reference_choice(enum Choice value) {
    if (value == ChoiceZero) return ChoiceTwo;
    return ChoiceZero;
}
int *reference_pointer(int *value) { *value += 7; return value; }
float reference_float(float value) { return value + 1.25f; }
double reference_double(double value) { return value + 2.5; }

_Bool ccc_boolean(_Bool);
signed char ccc_signed_char(signed char);
unsigned char ccc_unsigned_char(unsigned char);
short ccc_short(short);
unsigned short ccc_unsigned_short(unsigned short);
int ccc_int(int);
long ccc_long(long);
long long ccc_long_long(long long);
enum Choice ccc_choice(enum Choice);
int *ccc_pointer(int *);
float ccc_float(float);
double ccc_double(double);
int ccc_scalar_entry(void);

int main(void) {
    _Bool (*boolean_call)(_Bool) = ccc_boolean;
    unsigned char (*unsigned_char_call)(unsigned char) = ccc_unsigned_char;
    short (*short_call)(short) = ccc_short;
    int (*int_call)(int) = ccc_int;
    long long (*long_long_call)(long long) = ccc_long_long;
    int *(*pointer_call)(int *) = ccc_pointer;
    float (*float_call)(float) = ccc_float;
    int pointed = 9;
    int status = ccc_scalar_entry();
    if (status != 0) return status;
    if (boolean_call(0) != 1) return 113;
    if (ccc_signed_char(-5) != -4) return 114;
    if (unsigned_char_call(250) != 251) return 115;
    if (short_call(-300) != -298) return 116;
    if (ccc_unsigned_short(60000) != 60003) return 117;
    if (int_call(-1000) != -996) return 118;
    if (ccc_long(100000L) != 100005L) return 119;
    if (long_long_call(10000000000LL) != 10000000006LL) return 120;
    if (ccc_choice(ChoiceZero) != ChoiceTwo) return 121;
    if (pointer_call(&pointed) != &pointed || pointed != 16) return 122;
    if (float_call(1.5f) != 2.75f) return 123;
    if (ccc_double(3.5) != 6.0) return 124;
    return 0;
}
"#;

    for (compiler, optimization) in reference_configurations() {
        let directory = test_directory("scalar", compiler, optimization);
        let ccc_source = write(&directory, "ccc-scalar.c", CCC_SOURCE);
        let reference_source = write(&directory, "reference-scalar.c", REFERENCE_SOURCE);
        let ccc_object = directory.join("ccc-scalar.o");
        let reference_object = directory.join("reference-scalar.o");
        compile_ccc(compiler, &ccc_source, &ccc_object);
        compile_reference(compiler, optimization, &reference_source, &reference_object);
        let output = link_and_run(compiler, &directory, &[&ccc_object, &reference_object]);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
