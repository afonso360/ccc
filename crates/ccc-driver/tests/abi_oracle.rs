#![cfg(all(target_arch = "x86_64", target_os = "linux"))]

// This oracle compares the System V AMD64 classifier and x86-64 ELF layout
// with two independent compilers. Target-neutral and per-psABI evidence lives
// in the workspace tests and tests/target-oracle/run.sh.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ccc_target::{EffectiveCompilationConfig, Triple};
use object::{Architecture, Object as _, ObjectSection as _, ObjectSymbol as _};

mod support;

#[derive(Clone, Copy, Debug)]
enum CompilerFamily {
    Gcc,
    Clang,
}

#[derive(Clone, Debug)]
struct ReferenceCompiler {
    family: CompilerFamily,
    program: OsString,
    arguments: Vec<OsString>,
}

impl ReferenceCompiler {
    fn required(family: CompilerFamily, environment: &str, fallback: &str) -> Self {
        let value = std::env::var_os(environment).unwrap_or_else(|| OsString::from(fallback));
        let mut words = value
            .to_string_lossy()
            .split_whitespace()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert!(
            !words.is_empty(),
            "{environment} must name a compiler driver"
        );
        Self {
            family,
            program: words.remove(0),
            arguments: words,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.arguments);
        command
    }

    fn display(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.arguments
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn identity(&self) -> String {
        let output = self
            .command()
            .arg("--version")
            .output()
            .unwrap_or_else(|error| {
                panic!(
                    "required reference compiler `{}` is unavailable: {error}",
                    self.display()
                )
            });
        support::assert_command_success(
            &format!(
                "query the reference compiler identity for `{}`",
                self.display()
            ),
            &output,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let version = stdout
            .lines()
            .chain(stderr.lines())
            .find(|line| !line.trim().is_empty())
            .unwrap_or("<no version text>")
            .trim()
            .to_owned();
        let lowercase = version.to_ascii_lowercase();
        match self.family {
            CompilerFamily::Gcc => assert!(
                !lowercase.contains("clang")
                    && (lowercase.contains("gcc")
                        || lowercase.contains("free software foundation")),
                "`{}` is not a GCC driver: {version}",
                self.display()
            ),
            CompilerFamily::Clang => assert!(
                lowercase.contains("clang"),
                "`{}` is not a Clang driver: {version}",
                self.display()
            ),
        }
        format!("{} -- {version}", self.display())
    }

    fn target_triple(&self, identity: &str) -> Triple {
        let query = match self.family {
            CompilerFamily::Gcc => "-dumpmachine",
            CompilerFamily::Clang => "-print-target-triple",
        };
        let output = self
            .command()
            .arg(query)
            .output()
            .unwrap_or_else(|error| panic!("failed to query target for `{identity}`: {error}"));
        support::assert_command_success(
            &format!("query the reference compiler target for `{identity}`"),
            &output,
        );
        let text = String::from_utf8(output.stdout)
            .unwrap_or_else(|error| panic!("target query for `{identity}` was not UTF-8: {error}"));
        text.trim().parse().unwrap_or_else(|error| {
            panic!(
                "reference compiler `{identity}` reported invalid target triple `{}`: {error}",
                text.trim()
            )
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct SymbolBytes {
    size: u64,
    bytes: Vec<u8>,
}

const LAYOUT_FACTS: &[&str] = &[
    "sizeof(_Bool)",
    "_Alignof(_Bool)",
    "sizeof(char)",
    "_Alignof(char)",
    "sizeof(short)",
    "_Alignof(short)",
    "sizeof(int)",
    "_Alignof(int)",
    "sizeof(long)",
    "_Alignof(long)",
    "sizeof(long long)",
    "_Alignof(long long)",
    "sizeof(void *)",
    "_Alignof(void *)",
    "sizeof(float)",
    "_Alignof(float)",
    "sizeof(double)",
    "_Alignof(double)",
    "sizeof(long double)",
    "_Alignof(long double)",
    "sizeof(struct OracleRecord)",
    "_Alignof(struct OracleRecord)",
    "offsetof(struct OracleRecord, integer)",
    "offsetof(struct OracleRecord, wide)",
    "sizeof(union OracleUnion)",
    "_Alignof(union OracleUnion)",
    "sizeof(struct OracleAfterByte)",
    "_Alignof(struct OracleAfterByte)",
    "sizeof(struct OracleMixedTypes)",
    "_Alignof(struct OracleMixedTypes)",
    "sizeof(struct OracleSigned)",
    "_Alignof(struct OracleSigned)",
    "sizeof(struct OracleZeroWidth)",
    "_Alignof(struct OracleZeroWidth)",
    "sizeof(struct OracleZeroWidthOnly)",
    "_Alignof(struct OracleZeroWidthOnly)",
    "offsetof(struct OracleZeroWidthOnly, suffix)",
    "sizeof(struct OracleStraddling)",
    "_Alignof(struct OracleStraddling)",
    "sizeof(struct OracleAnonymous)",
    "_Alignof(struct OracleAnonymous)",
    "sizeof(struct OraclePlainInt)",
    "_Alignof(struct OraclePlainInt)",
    "offsetof(struct OraclePlainInt, tail)",
    "sizeof(struct OracleNestedInner)",
    "_Alignof(struct OracleNestedInner)",
    "sizeof(struct OracleNested)",
    "_Alignof(struct OracleNested)",
    "offsetof(struct OracleNested, inner)",
    "sizeof(union OracleBitfieldUnion)",
    "_Alignof(union OracleBitfieldUnion)",
    "sizeof(struct OraclePacked)",
    "_Alignof(struct OraclePacked)",
    "offsetof(struct OraclePacked, suffix)",
    "sizeof(struct OraclePackedZeroWidth)",
    "_Alignof(struct OraclePackedZeroWidth)",
    "offsetof(struct OraclePackedZeroWidth, suffix)",
];

const DELTA_GROUPS: &[(&str, &[&str])] = &[
    (
        "abi_after_byte_zero",
        &[
            "abi_after_byte_first_one",
            "abi_after_byte_first_max",
            "abi_after_byte_first_negative",
            "abi_after_byte_second_one",
            "abi_after_byte_second_max",
            "abi_after_byte_second_negative",
        ],
    ),
    (
        "abi_mixed_zero",
        &[
            "abi_mixed_byte_one",
            "abi_mixed_byte_max",
            "abi_mixed_half_one",
            "abi_mixed_half_max",
            "abi_mixed_word_one",
            "abi_mixed_word_max",
            "abi_mixed_wide_one",
            "abi_mixed_wide_max",
        ],
    ),
    (
        "abi_signed_zero",
        &[
            "abi_signed_negative_one",
            "abi_signed_negative_max",
            "abi_signed_negative_minus_one",
            "abi_signed_negative_min",
            "abi_signed_positive_one",
            "abi_signed_positive_max",
        ],
    ),
    (
        "abi_zero_width_zero",
        &[
            "abi_zero_width_low_one",
            "abi_zero_width_low_max",
            "abi_zero_width_high_one",
            "abi_zero_width_high_max",
        ],
    ),
    (
        "abi_straddling_zero",
        &[
            "abi_straddling_left_one",
            "abi_straddling_left_max",
            "abi_straddling_right_one",
            "abi_straddling_right_max",
        ],
    ),
    (
        "abi_anonymous_zero",
        &[
            "abi_anonymous_named_one",
            "abi_anonymous_named_max",
            "abi_anonymous_tail_one",
            "abi_anonymous_tail_max",
        ],
    ),
    (
        "abi_plain_zero",
        &[
            "abi_plain_signed_one",
            "abi_plain_signed_negative",
            "abi_plain_follower_max",
            "abi_plain_tail_one",
        ],
    ),
    (
        "abi_nested_zero",
        &[
            "abi_nested_prefix_one",
            "abi_nested_inner_left_one",
            "abi_nested_inner_left_max",
            "abi_nested_inner_right_negative",
            "abi_nested_tail_max",
        ],
    ),
    (
        "abi_bitfield_union_zero",
        &[
            "abi_bitfield_union_word_one",
            "abi_bitfield_union_word_max",
            "abi_bitfield_union_signed_negative",
            "abi_bitfield_union_byte_max",
        ],
    ),
    (
        "abi_packed_zero",
        &[
            "abi_packed_prefix_one",
            "abi_packed_low_one",
            "abi_packed_low_max",
            "abi_packed_high_one",
            "abi_packed_high_max",
            "abi_packed_suffix_one",
        ],
    ),
    (
        "abi_packed_zero_zero",
        &[
            "abi_packed_zero_low_max",
            "abi_packed_zero_high_max",
            "abi_packed_zero_suffix_one",
        ],
    ),
];

#[test]
fn x86_64_layout_objects_match_gcc_and_clang() {
    let expected = EffectiveCompilationConfig::default().target.triple;
    assert_eq!(
        expected.to_string(),
        "x86_64-unknown-linux-gnu",
        "the ABI oracle must follow CCC's enabled target"
    );

    let directory = support::TestWorkspace::new("abi-oracle", "layout-objects").retain_on_failure();
    let source = oracle_fixture("layout_objects.c");
    let ccc_object = directory.join("ccc.o");
    compile_ccc(&directory, &source, &ccc_object);
    let ccc_bytes = fs::read(&ccc_object).unwrap();
    let ccc_file = parse_x86_64_elf(&ccc_bytes, "CCC");

    for reference in [
        ReferenceCompiler::required(CompilerFamily::Gcc, "CCC_ABI_GCC", "gcc"),
        ReferenceCompiler::required(CompilerFamily::Clang, "CCC_ABI_CLANG", "clang"),
    ] {
        let identity = reference.identity();
        let actual = reference.target_triple(&identity);
        assert_matching_target(&expected, &actual, &identity);
        eprintln!("ABI layout oracle reference compiler: {identity}; target: {actual}");

        let family = match reference.family {
            CompilerFamily::Gcc => "gcc",
            CompilerFamily::Clang => "clang",
        };
        let reference_object = directory.join(format!("{family}.o"));
        compile_reference(
            &directory,
            &reference,
            &identity,
            &source,
            &reference_object,
        );
        let reference_bytes = fs::read(&reference_object).unwrap();
        let reference_file = parse_x86_64_elf(&reference_bytes, &identity);
        compare_oracle(&ccc_file, &reference_file, &identity);
    }
}

fn compile_ccc(directory: &support::TestWorkspace, source: &Path, output: &Path) {
    let result = support::ccc_command()
        .arg("-nostdinc")
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .expect("failed to run CCC for the ABI layout oracle");
    directory.assert_command_success("compile the ABI layout oracle with CCC", &result);
}

fn compile_reference(
    directory: &support::TestWorkspace,
    compiler: &ReferenceCompiler,
    identity: &str,
    source: &Path,
    output: &Path,
) {
    let result = compiler
        .command()
        .args([
            OsStr::new("-std=gnu11"),
            OsStr::new("-O0"),
            OsStr::new("-Wall"),
            OsStr::new("-Wextra"),
            OsStr::new("-Werror"),
            OsStr::new("-fno-common"),
            OsStr::new("-c"),
        ])
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap_or_else(|error| panic!("failed to run `{identity}`: {error}"));
    directory.assert_command_success(
        &format!("compile the ABI layout oracle with `{identity}`"),
        &result,
    );
}

fn parse_x86_64_elf<'a>(bytes: &'a [u8], identity: &str) -> object::File<'a> {
    assert!(
        bytes.starts_with(b"\x7fELF"),
        "compiler `{identity}` did not produce an ELF object"
    );
    let file = object::File::parse(bytes).unwrap();
    assert_eq!(
        file.architecture(),
        Architecture::X86_64,
        "compiler identity: {identity}"
    );
    file
}

fn assert_matching_target(expected: &Triple, actual: &Triple, identity: &str) {
    assert_eq!(
        actual.architecture, expected.architecture,
        "architecture mismatch for `{identity}`: `{actual}` versus `{expected}`"
    );
    assert_eq!(
        actual.operating_system, expected.operating_system,
        "operating-system mismatch for `{identity}`: `{actual}` versus `{expected}`"
    );
    assert_eq!(
        actual.environment, expected.environment,
        "environment mismatch for `{identity}`: `{actual}` versus `{expected}`"
    );
    assert_eq!(
        actual.binary_format, expected.binary_format,
        "object-format mismatch for `{identity}`: `{actual}` versus `{expected}`"
    );
}

fn compare_oracle(ccc: &object::File<'_>, reference: &object::File<'_>, identity: &str) {
    compare_layout_facts(ccc, reference, identity);

    for &(baseline, samples) in DELTA_GROUPS {
        for &sample in samples {
            assert_eq!(
                xor_from_baseline(ccc, baseline, sample),
                xor_from_baseline(reference, baseline, sample),
                "ABI object mismatch for `{sample}` relative to `{baseline}`; reference compiler: {identity}"
            );
        }
    }
}

fn compare_layout_facts(ccc: &object::File<'_>, reference: &object::File<'_>, identity: &str) {
    let ccc = symbol_bytes(ccc, "abi_layout_values");
    let reference = symbol_bytes(reference, "abi_layout_values");
    let expected_size = u64::try_from(LAYOUT_FACTS.len()).unwrap() * 8;
    assert_eq!(ccc.size, expected_size, "CCC layout table is malformed");
    assert_eq!(
        reference.size, expected_size,
        "reference layout table is malformed: {identity}"
    );
    for (index, fact) in LAYOUT_FACTS.iter().enumerate() {
        let start = index * 8;
        let end = start + 8;
        let ccc_value = u64::from_le_bytes(ccc.bytes[start..end].try_into().unwrap());
        let reference_value = u64::from_le_bytes(reference.bytes[start..end].try_into().unwrap());
        assert_eq!(
            ccc_value, reference_value,
            "ABI layout mismatch for `{fact}`; reference compiler: {identity}"
        );
    }
}

fn xor_from_baseline(file: &object::File<'_>, baseline: &str, sample: &str) -> SymbolBytes {
    let baseline_bytes = symbol_bytes(file, baseline);
    let sample_bytes = symbol_bytes(file, sample);
    assert_eq!(
        sample_bytes.size, baseline_bytes.size,
        "`{sample}` and its zero baseline have different sizes"
    );
    assert_eq!(
        sample_bytes.bytes.len(),
        baseline_bytes.bytes.len(),
        "`{sample}` and its zero baseline have different byte lengths"
    );
    SymbolBytes {
        size: sample_bytes.size,
        bytes: sample_bytes
            .bytes
            .into_iter()
            .zip(baseline_bytes.bytes)
            .map(|(sample, baseline)| sample ^ baseline)
            .collect(),
    }
}

fn symbol_bytes(file: &object::File<'_>, name: &str) -> SymbolBytes {
    let symbol = file
        .symbols()
        .find(|symbol| symbol.name() == Ok(name) && symbol.is_definition())
        .unwrap_or_else(|| panic!("object has no defined `{name}` symbol"));
    let section_index = symbol
        .section_index()
        .unwrap_or_else(|| panic!("symbol `{name}` has no containing section"));
    let section = file.section_by_index(section_index).unwrap();
    let offset = symbol
        .address()
        .checked_sub(section.address())
        .and_then(|offset| usize::try_from(offset).ok())
        .unwrap_or_else(|| panic!("symbol `{name}` has an invalid section offset"));
    let size = usize::try_from(symbol.size())
        .unwrap_or_else(|_| panic!("symbol `{name}` is too large to inspect"));
    let end = offset
        .checked_add(size)
        .unwrap_or_else(|| panic!("symbol `{name}` range overflows"));
    let section_data = section.data().unwrap();
    let bytes = section_data
        .get(offset..end)
        .unwrap_or_else(|| panic!("symbol `{name}` extends beyond its section"))
        .to_vec();
    SymbolBytes {
        size: symbol.size(),
        bytes,
    }
}

fn oracle_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/abi-oracle")
        .join(name)
}
