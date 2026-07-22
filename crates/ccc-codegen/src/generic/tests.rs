use std::collections::BTreeSet;

use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, RelocationEncoding, RelocationFlags,
    RelocationKind, RelocationTarget,
};
use sha2::{Digest as _, Sha256};

use super::*;

fn lower_source_with_config(source: &str, config: &EffectiveCompilationConfig) -> gir::FullModule {
    lower_source_with_map(source, config).0
}

fn lower_source_with_map(
    source: &str,
    config: &EffectiveCompilationConfig,
) -> (gir::FullModule, SourceMap) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("generic-codegen-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config)
        .unwrap_or_else(|diagnostics| panic!("semantic diagnostics: {diagnostics:#?}"));
    let module = gir::lower_frontend(&typed).unwrap();
    gir::verify_frontend(&module).unwrap();
    (module, sources)
}

fn lower_source(source: &str) -> gir::FullModule {
    lower_source_with_config(source, &EffectiveCompilationConfig::default())
}

#[test]
fn compiler_128_bit_storage_is_target_gated() {
    for (config, symbol_name) in [
        (EffectiveCompilationConfig::default(), "wide_object"),
        (
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            "wide_object",
        ),
        (
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            "wide_object",
        ),
        (
            EffectiveCompilationConfig::aarch64_apple_darwin(),
            "_wide_object",
        ),
    ] {
        let output = emit_source_with_config(
            "__int128 wide_object; void *address(void) { return &wide_object; }",
            &config,
        );
        let object = object::File::parse(output.object.as_slice()).unwrap();
        let symbol = object.symbol_by_name(symbol_name).unwrap();
        if object.format() == object::BinaryFormat::MachO {
            let section = object
                .section_by_index(symbol.section_index().unwrap())
                .unwrap();
            assert!(section.size() >= 16, "{}", config.target.triple);
        } else {
            assert_eq!(symbol.size(), 16, "{}", config.target.triple);
        }

        let types = TypeStore::default();
        for ty in [TypeId::INT128, TypeId::UNSIGNED_INT128] {
            let lowered =
                super::function::scalar_type(&types, QualifiedType::unqualified(ty), &config);
            if config.target.abi.supports_int128_values() {
                assert_eq!(lowered.unwrap(), ir::types::I128);
            } else {
                assert_eq!(lowered.unwrap_err().code, "CCC3517");
            }
        }
    }
}

#[test]
fn wide_operations_select_only_manifested_helpers_that_are_actually_used() {
    let output = emit_source(
        "typedef __int128 i128; typedef unsigned __int128 u128;\n\
         i128 add(i128 a, i128 b) { return a + b; }\n\
         u128 multiply(u128 a, u128 b) { return a * b; }\n\
         i128 shift(i128 a, unsigned b) { return (a << b) ^ (a >> b); }\n\
         int compare(u128 a, u128 b) { return a < b; }\n\
         i128 divide(i128 a, i128 b) { return a / b; }\n\
         u128 remainder(u128 a, u128 b) { return a % b; }\n\
         double to_double(i128 a) { return (double)a; }\n\
         u128 from_float(float a) { return (u128)a; }",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let undefined = object
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| symbol.name().ok())
        .filter(|symbol| symbol.starts_with("__"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        undefined,
        ["__divti3", "__fixunssfti", "__floattidf", "__umodti3"]
            .into_iter()
            .collect()
    );
    for forbidden in [
        "__multi3",
        "__ashlti3",
        "__ashrti3",
        "__lshrti3",
        "__cmpti2",
        "__ucmpti2",
    ] {
        assert!(!undefined.contains(forbidden));
    }
}

#[test]
fn compiler_float16_storage_emits_but_scalar_lowering_fails_closed() {
    for (config, symbol_name) in [
        (EffectiveCompilationConfig::default(), "half_object"),
        (
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            "half_object",
        ),
        (
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            "half_object",
        ),
        (
            EffectiveCompilationConfig::aarch64_apple_darwin(),
            "_half_object",
        ),
    ] {
        let output = emit_source_with_config(
            "_Float16 half_object; void *address(void) { return &half_object; }",
            &config,
        );
        let object = object::File::parse(output.object.as_slice()).unwrap();
        let symbol = object.symbol_by_name(symbol_name).unwrap();
        if object.format() == object::BinaryFormat::MachO {
            let section = object
                .section_by_index(symbol.section_index().unwrap())
                .unwrap();
            assert!(section.size() >= 2, "{}", config.target.triple);
        } else {
            assert_eq!(symbol.size(), 2, "{}", config.target.triple);
        }

        let types = TypeStore::default();
        let error = super::function::scalar_type(
            &types,
            QualifiedType::unqualified(TypeId::FLOAT16),
            &config,
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC3518", "{}", config.target.triple);
    }
}

#[test]
fn wide_static_initializers_and_bitfields_preserve_high_bytes() {
    let output = emit_source(
        "typedef unsigned __int128 u128;\n\
         u128 whole = 0xffffffffffffffffffffffffffffffffU;\n\
         struct Bits { u128 low : 80; u128 high : 48; };\n\
         struct Bits bits = {\n\
           .low = 0xabcdeffffffffffedcbaU,\n\
           .high = 0x123456789abcU,\n\
         };",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let whole = object.symbol_by_name("whole").unwrap();
    let section = object
        .section_by_index(whole.section_index().unwrap())
        .unwrap();
    let whole_offset = whole.address() - section.address();
    assert_eq!(
        &section.data().unwrap()[whole_offset as usize..whole_offset as usize + 16],
        &[0xff; 16]
    );
    let bits = object.symbol_by_name("bits").unwrap();
    let section = object
        .section_by_index(bits.section_index().unwrap())
        .unwrap();
    let offset = (bits.address() - section.address()) as usize;
    assert_eq!(
        &section.data().unwrap()[offset..offset + 16],
        &[
            0xba, 0xdc, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xef, 0xcd, 0xab, 0xbc, 0x9a, 0x78, 0x56,
            0x34, 0x12,
        ]
    );
}

#[test]
fn wide_integer_float_constants_round_at_float_precision() {
    let output = emit_source(
        "typedef __int128 i128;\n\
         float from_wide = (float)(((i128)1 << 100) + ((i128)1 << 76) + 1);\n\
         i128 from_literal = (i128)16777217.0f;\n\
         i128 from_expression = (i128)(16777216.0f + 1.0f);\n\
         float folded_division = 1.0f / 10.0f;\n\
         unsigned __int128 unsigned_high =\n\
             (unsigned __int128)170141183460469231731687303715884105728.0;\n\
         unsigned __int128 unsigned_near_max =\n\
             (unsigned __int128)0x1.fffffffffffffp127;",
    );
    assert_eq!(
        symbol_bytes(&output.object, "from_wide"),
        0x7180_0001_u32.to_le_bytes()
    );
    let mut expected = [0_u8; 16];
    expected[..8].copy_from_slice(&16_777_216_u64.to_le_bytes());
    assert_eq!(symbol_bytes(&output.object, "from_literal"), expected);
    assert_eq!(symbol_bytes(&output.object, "from_expression"), expected);
    assert_eq!(
        symbol_bytes(&output.object, "folded_division"),
        0.1_f32.to_le_bytes()
    );
    let mut unsigned_high = [0_u8; 16];
    unsigned_high[15] = 0x80;
    assert_eq!(symbol_bytes(&output.object, "unsigned_high"), unsigned_high);
    let unsigned_near_max = (0_u128.wrapping_sub(1_u128 << 75)).to_le_bytes();
    assert_eq!(
        symbol_bytes(&output.object, "unsigned_near_max"),
        unsigned_near_max
    );
}

#[test]
fn unused_float16_sdk_prototypes_do_not_require_value_transport() {
    let source = "extern _Float16 __fabsf16(_Float16);\n\
                  extern _Float16 __fmaf16(_Float16, _Float16, _Float16);\n\
                  int answer(void) { return 42; }";
    let output =
        emit_source_with_config(source, &EffectiveCompilationConfig::aarch64_apple_darwin());
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(object.symbol_by_name("_answer").is_some());
    assert!(object.symbol_by_name("___fabsf16").unwrap().is_undefined());
    assert!(object.symbol_by_name("___fmaf16").unwrap().is_undefined());
}

#[test]
fn float16_value_transport_fails_with_one_stable_diagnostic() {
    let config = EffectiveCompilationConfig::aarch64_apple_darwin();
    for source in [
        "_Float16 initialized = 1.0;",
        "_Float16 defined(_Float16 value) { return value; }",
        "extern _Float16 operation(_Float16); int call(void) { return operation(1.0) != 0; }",
        "int arithmetic(void) { _Float16 value; return value + value != 0; }",
        "typedef __builtin_va_list va_list; int read(int count, ...) { va_list list; __builtin_va_start(list, count); return __builtin_va_arg(list, _Float16) != 0; }",
    ] {
        let module = lower_source_with_config(source, &config);
        let error = emit(
            &module,
            &config,
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC3518", "{source}: {error}");
        assert!(error.message.contains("_Float16"), "{source}: {error}");
    }
}

fn emit_source(source: &str) -> Output {
    emit(
        &lower_source(source),
        &EffectiveCompilationConfig::default(),
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap()
}

fn emit_source_with_config(source: &str, config: &EffectiveCompilationConfig) -> Output {
    emit(
        &lower_source_with_config(source, config),
        config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap()
}

#[test]
fn indirect_calls_select_the_conservative_returns_twice_codegen_profile() {
    let indirect = lower_source(
        "int invoke(int (*callback)(int), int value) {\n\
             int retained = value + 1;\n\
             return callback(value) + retained;\n\
         }",
    );
    assert!(module_contains_returns_twice_call(&indirect));
    assert!(indirect.functions[0].storage.iter().any(|storage| {
        storage
            .required_by
            .contains(&gir::MemoryResidencyReason::ReturnsTwice)
    }));
    let output = emit(
        &indirect,
        &EffectiveCompilationConfig::default(),
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    assert!(!output.object.is_empty());

    let direct = lower_source(
        "int ordinary(int value) { return value + 1; }\n\
         int invoke(int value) { return ordinary(value); }",
    );
    assert!(!module_contains_returns_twice_call(&direct));
}

#[test]
fn enabled_non_x86_targets_emit_native_objects_with_fixed_aggregate_calls() {
    let source = "struct Pair { long first, second; };\n\
                  struct Floats { double first, second; };\n\
                  struct Pair swap(struct Pair value) {\n\
                    struct Pair result = { value.second, value.first }; return result;\n\
                  }\n\
                  double sum(struct Floats value) { return value.first + value.second; }";
    for (config, format, architecture) in [
        (
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            object::BinaryFormat::Elf,
            object::Architecture::Aarch64,
        ),
        (
            EffectiveCompilationConfig::aarch64_apple_darwin(),
            object::BinaryFormat::MachO,
            object::Architecture::Aarch64,
        ),
        (
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            object::BinaryFormat::Elf,
            object::Architecture::Riscv64,
        ),
    ] {
        let output = emit_source_with_config(source, &config);
        assert!(output.assemblies.is_empty());
        let object = object::File::parse(output.object.as_slice()).unwrap();
        assert_eq!(object.format(), format);
        assert_eq!(object.architecture(), architecture);
        if architecture == object::Architecture::Riscv64 {
            assert_eq!(
                object.flags(),
                object::FileFlags::Elf {
                    os_abi: object::elf::ELFOSABI_NONE,
                    abi_version: 0,
                    e_flags: object::elf::EF_RISCV_RVC | object::elf::EF_RISCV_FLOAT_ABI_DOUBLE,
                }
            );
        }
        let prefix = if format == object::BinaryFormat::MachO {
            "_"
        } else {
            ""
        };
        let swap = format!("{prefix}swap");
        let sum = format!("{prefix}sum");
        assert!(
            object
                .symbols()
                .any(|symbol| symbol.name() == Ok(swap.as_str())),
            "{} expected {swap}; symbols={:?}",
            config.target.triple,
            object
                .symbols()
                .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
                .collect::<Vec<_>>()
        );
        assert!(
            object
                .symbols()
                .any(|symbol| symbol.name() == Ok(sum.as_str()))
        );
    }
}

#[test]
fn darwin_symbols_tentative_data_and_libcalls_match_apple_spelling() {
    let output = emit_source_with_config(
        "int tentative;\n\
         int default_function(void) { return tentative; }\n\
         int hidden_function(void) __attribute__((visibility(\"hidden\")));\n\
         int hidden_function(void) { return 2; }\n\
         int protected_function(void) __attribute__((visibility(\"protected\")));\n\
         int protected_function(void) { return 3; }\n\
         int internal_function(void) __attribute__((visibility(\"internal\")));\n\
         int internal_function(void) { return 4; }\n\
         void copy_bytes(void *to, const void *from, unsigned long count) {\n\
             __builtin_memcpy(to, from, count);\n\
         }",
        &EffectiveCompilationConfig::aarch64_apple_darwin(),
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let tentative = object.symbol_by_name("_tentative").unwrap();
    assert!(!tentative.is_undefined());
    assert_ne!(tentative.section(), object::SymbolSection::Common);
    for (name, scope) in [
        ("_default_function", object::SymbolScope::Dynamic),
        ("_hidden_function", object::SymbolScope::Linkage),
        ("_protected_function", object::SymbolScope::Dynamic),
        ("_internal_function", object::SymbolScope::Linkage),
    ] {
        assert_eq!(
            object.symbol_by_name(name).unwrap().scope(),
            scope,
            "{name}"
        );
    }
    let memcpy = object.symbol_by_name("_memcpy").unwrap();
    assert!(memcpy.is_undefined());
    assert!(object.symbol_by_name("__memcpy").is_none());
}

#[test]
fn darwin_emits_text_before_data_for_linker_unwind_conversion() {
    let output = emit_source_with_config(
        "int first(void) { return \"x\"[0]; }\n\
         int main(void) { return first() == 'x' ? 0 : 1; }",
        &EffectiveCompilationConfig::aarch64_apple_darwin(),
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let section_names = object
        .sections()
        .map(|section| section.name().unwrap().to_owned())
        .collect::<Vec<_>>();
    let text = section_names
        .iter()
        .position(|name| name == "__text")
        .expect("Darwin text section");
    let data = section_names
        .iter()
        .position(|name| name == "__const")
        .expect("Darwin constant-data section");
    assert!(
        text < data,
        "Apple's linker requires text before data when deriving compact unwind: {section_names:?}"
    );
}

#[test]
fn arm64_fixed_aggregate_exhaustion_emits_complete_stack_transports() {
    const INTEGER_SOURCE: &str = "struct Pair { long first; long second; };\n\
         long pair_after_seven(long a0, long a1, long a2, long a3, long a4, long a5, long a6, struct Pair value) {\n\
         return a0+a1+a2+a3+a4+a5+a6+value.first+value.second; }\n\
         long invoke(void) { struct Pair pair = {8, 9};\n\
           return pair_after_seven(1,2,3,4,5,6,7,pair); }";
    const HFA_SOURCE: &str = "struct Hfa { double first; double second; };\n\
         long hfa_after_seven(double a0, double a1, double a2, double a3, double a4, double a5, double a6, struct Hfa value) {\n\
           return (long)(a0+a1+a2+a3+a4+a5+a6+value.first+value.second); }";
    let integer_module = lower_source(INTEGER_SOURCE);
    let hfa_module = lower_source(HFA_SOURCE);
    for config in [
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        let plan = ccc_abi::plan_module(&integer_module, &config).unwrap();
        assert!(plan.definitions.values().any(|definition| {
            let ccc_abi::BoundaryPlan::Native(native) = &definition.boundary else {
                return false;
            };
            native
                .clif_parameters
                .iter()
                .any(|carrier| carrier.purpose == ccc_abi::NativePurpose::Padding)
        }));
        let output = emit(&integer_module, &config, Options::default()).unwrap();
        let object = object::File::parse(output.object.as_slice()).unwrap();
        assert_eq!(object.architecture(), object::Architecture::Aarch64);

        let plan = ccc_abi::plan_module(&hfa_module, &config).unwrap();
        assert!(plan.definitions.values().any(|definition| {
            let ccc_abi::BoundaryPlan::Native(native) = &definition.boundary else {
                return false;
            };
            native
                .clif_parameters
                .iter()
                .any(|carrier| carrier.purpose == ccc_abi::NativePurpose::Padding)
        }));
        let output = emit(&hfa_module, &config, Options::default()).unwrap();
        let object = object::File::parse(output.object.as_slice()).unwrap();
        assert_eq!(object.architecture(), object::Architecture::Aarch64);
    }
}

#[test]
fn enabled_non_x86_targets_plan_target_variadics_and_emit_matching_adapters() {
    let source = "typedef __builtin_va_list va_list;\n\
                  struct Pair { long first, second; };\n\
                  long collect(int count, ...) {\n\
                    va_list list; struct Pair pair; long integer; double floating;\n\
                    __builtin_va_start(list, count);\n\
                    integer = __builtin_va_arg(list, long);\n\
                    floating = __builtin_va_arg(list, double);\n\
                    pair = __builtin_va_arg(list, struct Pair);\n\
                    __builtin_va_end(list);\n\
                    return integer + (long)floating + pair.first + pair.second;\n\
                  }\n\
                  long invoke(void) { struct Pair pair = { 3, 4 };\n\
                    return collect(3, 1L, 2.0, pair); }";
    for (config, expected) in [
        (
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            "str q7, [sp, #224]",
        ),
        (
            EffectiveCompilationConfig::aarch64_apple_darwin(),
            ".subsections_via_symbols",
        ),
        (
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            "sd a7, 504(sp)",
        ),
    ] {
        let output = emit_source_with_config(source, &config);
        assert_eq!(output.assemblies.len(), 2);
        assert!(
            output
                .assemblies
                .iter()
                .any(|assembly| assembly.source().contains(expected)),
            "{} adapter did not contain `{expected}`",
            config.target.triple
        );
        assert!(
            output
                .manifest
                .symbols()
                .iter()
                .any(|symbol| symbol.kind == ccc_link::bridge::GeneratedSymbolKind::CallHelper)
        );
    }
}

#[test]
fn riscv_variadic_entry_keeps_fixed_float_arguments_separate_from_results() {
    let config = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
    let output = emit_source_with_config(
        "typedef __builtin_va_list va_list;\n\
         long collect(double first, double second, int count, ...) {\n\
           va_list list; long tail; __builtin_va_start(list, count);\n\
           tail = __builtin_va_arg(list, long);\n\
           return (long)first + (long)second + tail; }",
        &config,
    );
    let entry = output
        .assemblies
        .iter()
        .find(|assembly| assembly.stem().starts_with("variadic-entry-"))
        .expect("RISC-V variadic definition adapter");
    assert!(entry.source().contains("fsd fa0, 112(sp)"));
    assert!(entry.source().contains("fsd fa1, 128(sp)"));
    assert!(!entry.source().contains("fsd fa0, 288(sp)"));
    assert!(entry.source().contains("sd zero, 288(sp)"));
}

#[test]
fn non_x86_tls_is_rejected_before_backend_lowering() {
    let module = lower_source(
        "_Thread_local int value;\n\
         int read(void) { return value; }",
    );
    for config in [
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        let error = emit(&module, &config, Options::default()).unwrap_err();
        assert_eq!(error.code, "CCC3522");
        assert!(error.message.contains(config.target.abi.name()));
    }
}

#[test]
fn darwin_binary64_long_double_uses_the_double_transport() {
    let config = EffectiveCompilationConfig::aarch64_apple_darwin();
    let output = emit_source_with_config(
        "typedef __builtin_va_list va_list;\n\
         long double identity(long double value) { return value; }\n\
         long double call_identity(long double value) { return identity(value); }\n\
         long double read(int count, ...) { va_list list;\n\
           __builtin_va_start(list, count);\n\
           return __builtin_va_arg(list, long double); }",
        &config,
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert_eq!(object.format(), object::BinaryFormat::MachO);
    assert!(
        output
            .assemblies
            .iter()
            .any(|assembly| { assembly.source().contains(".subsections_via_symbols") })
    );
}

#[test]
fn darwin_declaration_assembly_labels_are_exact_physical_symbols() {
    let config = EffectiveCompilationConfig::aarch64_apple_darwin();
    let output = emit_source_with_config(
        "extern int source_function(int) asm(\"_external_function\");\n\
         extern int source_object asm(\"external_object\");\n\
         int defined_function(int) asm(\"_defined_function\");\n\
         int defined_function(int value) { return source_function(value) + source_object; }\n\
         int defined_object asm(\"defined_object\") = 7;\n\
         int _ordinary_leading(void) { return defined_object; }\n\
         int ordinary_object;",
        &config,
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert_eq!(object.format(), object::BinaryFormat::MachO);

    for (name, kind, defined) in [
        ("_external_function", object::SymbolKind::Unknown, false),
        ("external_object", object::SymbolKind::Unknown, false),
        ("_defined_function", object::SymbolKind::Text, true),
        ("defined_object", object::SymbolKind::Data, true),
        ("__ordinary_leading", object::SymbolKind::Text, true),
        ("_ordinary_object", object::SymbolKind::Data, true),
    ] {
        let symbol = object
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing Mach-O symbol `{name}`"));
        assert_eq!(symbol.kind(), kind, "{name}");
        assert_eq!(symbol.is_definition(), defined, "{name}");
    }
    for incorrectly_mangled in [
        "__external_function",
        "_external_object",
        "__defined_function",
        "_defined_object",
        "_ordinary_leading",
        "ordinary_object",
    ] {
        assert!(
            object.symbol_by_name(incorrectly_mangled).is_none(),
            "unexpected Mach-O symbol `{incorrectly_mangled}`"
        );
    }

    let relocation_targets = object
        .sections()
        .flat_map(|section| section.relocations())
        .filter_map(|(_, relocation)| match relocation.target() {
            object::RelocationTarget::Symbol(index) => object
                .symbol_by_index(index)
                .ok()
                .and_then(|symbol| symbol.name().ok())
                .map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>();
    for name in ["_external_function", "external_object", "defined_object"] {
        assert!(
            relocation_targets.iter().any(|target| target == name),
            "missing relocation to `{name}`: {relocation_targets:?}"
        );
    }
}

#[test]
fn darwin_variadic_bridge_preserves_exact_public_assembly_label() {
    let config = EffectiveCompilationConfig::aarch64_apple_darwin();
    let output = emit_source_with_config(
        "int source_variadic(int fixed, ...) asm(\"physical_variadic\");\n\
         int source_variadic(int fixed, ...) { return fixed; }\n\
         int invoke(void) { return source_variadic(7); }",
        &config,
    );

    let primary = object::File::parse(output.object.as_slice()).unwrap();
    let entry = primary.symbol_by_name("physical_variadic").unwrap();
    assert!(entry.is_undefined());
    assert!(primary.symbol_by_name("_physical_variadic").is_none());

    let assembly = output
        .assemblies
        .iter()
        .find(|assembly| assembly.stem().starts_with("variadic-entry-"))
        .expect("Darwin variadic entry assembly");
    assert!(assembly.source().contains(".globl physical_variadic\n"));
    assert!(assembly.source().contains("physical_variadic:\n"));
    assert!(!assembly.source().contains("_physical_variadic"));

    let manifest_entry = output
        .manifest
        .symbols()
        .iter()
        .find(|symbol| symbol.name == "physical_variadic")
        .expect("variadic entry manifest symbol");
    assert!(manifest_entry.object_name_is_exact);
    output.into_artifact_bundle().verify().unwrap();
}

#[test]
fn emitted_functions_have_relocatable_system_v_call_frames() {
    use gimli::UnwindSection as _;

    let output = emit_source(
        "int first(int value) { return value + 1; }\n\
         long second(long value) { return first((int)value) + 2; }",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let section = object
        .section_by_name(".eh_frame")
        .expect("System V call-frame section");
    assert_eq!(
        section.kind(),
        object::SectionKind::Elf(object::elf::SHT_X86_64_UNWIND)
    );
    assert!(
        matches!(
            section.flags(),
            object::SectionFlags::Elf { sh_flags }
                if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0
        ),
        "`.eh_frame` must be allocated"
    );

    let mut eh_frame = gimli::EhFrame::new(section.data().unwrap(), gimli::LittleEndian);
    eh_frame.set_address_size(8);
    let bases = gimli::BaseAddresses::default();
    let mut entries = eh_frame.entries(&bases);
    let (mut cies, mut fdes) = (0, 0);
    while let Some(entry) = entries.next().unwrap() {
        match entry {
            gimli::CieOrFde::Cie(_) => cies += 1,
            gimli::CieOrFde::Fde(_) => fdes += 1,
        }
    }
    assert_eq!(cies, 1);
    assert_eq!(fdes, 2);

    let text = object.section_by_name(".text").unwrap().index();
    let mut expected_addends = ["first", "second"]
        .map(|name| {
            i64::try_from(
                object
                    .symbols()
                    .find(|symbol| symbol.name() == Ok(name))
                    .unwrap()
                    .address(),
            )
            .unwrap()
        })
        .to_vec();
    expected_addends.sort_unstable();

    let relocations = section.relocations().collect::<Vec<_>>();
    assert_eq!(relocations.len(), 2);
    let mut actual_addends = Vec::with_capacity(relocations.len());
    for (_, relocation) in relocations {
        assert_eq!(relocation.kind(), RelocationKind::Relative);
        assert_eq!(relocation.encoding(), RelocationEncoding::Generic);
        assert_eq!(relocation.size(), 32);
        let RelocationTarget::Symbol(symbol) = relocation.target() else {
            panic!("call-frame relocation must target the text section")
        };
        let symbol = object.symbol_by_index(symbol).unwrap();
        assert_eq!(symbol.kind(), object::SymbolKind::Section);
        assert_eq!(symbol.section_index(), Some(text));
        actual_addends.push(relocation.addend());
    }
    actual_addends.sort_unstable();
    assert_eq!(actual_addends, expected_addends);
}

#[test]
fn function_visibility_reaches_native_objects_and_variadic_assembly() {
    let native = emit_source(
        "int native_default(void) __attribute__((visibility(\"default\")));\n\
         int native_default(void) { return 1; }\n\
         int native_hidden(void) __attribute__((visibility(\"hidden\")));\n\
         int native_hidden(void) { return 2; }\n\
         int native_protected(void) __attribute__((visibility(\"protected\")));\n\
         int native_protected(void) { return 3; }\n\
         int native_internal(void) __attribute__((visibility(\"internal\")));\n\
         int native_internal(void) { return 4; }\n\
         int global_default __attribute__((visibility(\"default\"))) = 1;\n\
         int global_hidden __attribute__((visibility(\"hidden\"))) = 2;\n\
         int global_protected __attribute__((visibility(\"protected\"))) = 3;\n\
         int global_internal __attribute__((visibility(\"internal\"))) = 4;\n\
         int undefined_default(void) __attribute__((visibility(\"default\")));\n\
         int undefined_hidden(void) __attribute__((visibility(\"hidden\")));\n\
         int undefined_protected(void) __attribute__((visibility(\"protected\")));\n\
         int undefined_internal(void) __attribute__((visibility(\"internal\")));\n\
         int retain_undefined(void) {\n\
             return undefined_default() + undefined_hidden()\n\
                 + undefined_protected() + undefined_internal();\n\
         }",
    );
    let object = object::File::parse(native.object.as_slice()).unwrap();
    for prefix in ["native", "global", "undefined"] {
        for (suffix, visibility) in [
            ("default", object::elf::STV_DEFAULT),
            ("hidden", object::elf::STV_HIDDEN),
            ("protected", object::elf::STV_PROTECTED),
            ("internal", object::elf::STV_INTERNAL),
        ] {
            let name = format!("{prefix}_{suffix}");
            let symbol = object
                .symbols()
                .find(|symbol| symbol.name() == Ok(name.as_str()))
                .unwrap_or_else(|| panic!("missing `{name}`"));
            assert_ne!(symbol.scope(), object::SymbolScope::Compilation, "{name}");
            assert_eq!(symbol.flags().elf_visibility(), Some(visibility), "{name}");
            assert_eq!(symbol.is_undefined(), prefix == "undefined", "{name}");
        }
    }

    let variadic = emit_source(
        "int variadic_default(int marker, ...) __attribute__((visibility(\"default\")));\n\
         int variadic_default(int marker, ...) { return marker; }\n\
         int variadic_hidden(int marker, ...) __attribute__((visibility(\"hidden\")));\n\
         int variadic_hidden(int marker, ...) { return marker; }\n\
         int variadic_protected(int marker, ...) __attribute__((visibility(\"protected\")));\n\
         int variadic_protected(int marker, ...) { return marker; }\n\
         int variadic_internal(int marker, ...) __attribute__((visibility(\"internal\")));\n\
         int variadic_internal(int marker, ...) { return marker; }",
    );
    for (suffix, visibility, directive) in [
        (
            "default",
            ccc_link::artifact::GeneratedSymbolVisibility::Public,
            None,
        ),
        (
            "hidden",
            ccc_link::artifact::GeneratedSymbolVisibility::SourceHidden,
            Some(".hidden variadic_hidden"),
        ),
        (
            "protected",
            ccc_link::artifact::GeneratedSymbolVisibility::SourceProtected,
            Some(".protected variadic_protected"),
        ),
        (
            "internal",
            ccc_link::artifact::GeneratedSymbolVisibility::SourceElfInternal,
            Some(".internal variadic_internal"),
        ),
    ] {
        let name = format!("variadic_{suffix}");
        let entry = variadic
            .manifest
            .symbols()
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap();
        assert_eq!(entry.visibility, visibility, "{name}");
        let assembly = variadic
            .assemblies
            .iter()
            .find(|assembly| {
                assembly
                    .defined_symbols()
                    .iter()
                    .any(|symbol| symbol == &name)
            })
            .unwrap();
        for possible in [".hidden ", ".protected ", ".internal "] {
            let rendered = format!("{possible}{name}");
            assert_eq!(
                assembly.source().contains(&rendered),
                directive == Some(rendered.as_str()),
                "{name}:\n{}",
                assembly.source()
            );
        }
    }
}

#[test]
fn weak_binding_reaches_defined_and_undefined_elf_symbols() {
    const SOURCE: &str = "extern int weak_import(void) __attribute__((__weak__));\n\
         extern int weak_data_import __attribute__((weak));\n\
         int weak_function(void) __attribute__((weak));\n\
         int weak_function(void) { return 7; }\n\
         int weak_data __attribute__((weak)) = 11;\n\
         int weak_zero __attribute__((weak));\n\
         int strong_function(void) { return 13; }\n\
         int retain_imports(void) { return weak_import() + weak_data_import; }";
    let module = lower_source(SOURCE);
    assert_eq!(
        module
            .functions
            .iter()
            .find(|function| function.name == "weak_function")
            .unwrap()
            .binding,
        SymbolBinding::Weak
    );
    assert_eq!(
        module
            .globals
            .iter()
            .find(|global| global.name == "weak_data")
            .unwrap()
            .emission
            .binding,
        SymbolBinding::Weak
    );
    let output = emit(
        &module,
        &EffectiveCompilationConfig::default(),
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    let object = object::File::parse(output.object.as_slice()).unwrap();
    for (name, undefined) in [
        ("weak_import", true),
        ("weak_data_import", true),
        ("weak_function", false),
        ("weak_data", false),
        ("weak_zero", false),
    ] {
        let symbol = object
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .unwrap_or_else(|| panic!("missing `{name}`"));
        assert!(symbol.is_weak(), "{name}");
        assert_eq!(symbol.is_undefined(), undefined, "{name}");
    }
    assert!(
        object
            .symbol_by_name("strong_function")
            .unwrap()
            .is_global()
    );
    assert!(!object.symbol_by_name("strong_function").unwrap().is_weak());
    let weak_zero = object.symbol_by_name("weak_zero").unwrap();
    let section = object
        .section_by_index(weak_zero.section_index().expect("weak definition section"))
        .unwrap();
    assert_eq!(section.kind(), object::SectionKind::UninitializedData);
}

#[test]
fn thread_local_accesses_use_manifested_generated_accessors() {
    const SOURCE: &str = "_Thread_local int external_value = 7;\n\
         int read_values(void) {\n\
             static _Thread_local int block_value = 5;\n\
             ++external_value;\n\
             return external_value + block_value;\n\
         }";
    let module = lower_source(SOURCE);
    let config = EffectiveCompilationConfig::default();
    let plan = ccc_abi::plan_module(&module, &config).unwrap();
    assert_eq!(plan.artifacts.tls_accessors.len(), 2);
    assert_eq!(
        plan.artifacts.packaging.generated_assembly_units, 2,
        "one deterministic assembly unit is planned per TLS object"
    );
    let first_dump = ccc_abi::dump_module_plan(plan.verify_against(&module, &config).unwrap());
    let second = ccc_abi::plan_module(&module, &config).unwrap();
    assert_eq!(
        first_dump,
        ccc_abi::dump_module_plan(second.verify_against(&module, &config).unwrap())
    );

    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    assert!(!output.clif.contains("tls_value"), "{}", output.clif);
    assert_eq!(output.assemblies.len(), 2);
    assert_eq!(
        output
            .manifest
            .symbols()
            .iter()
            .filter(|symbol| symbol.kind == ccc_link::bridge::GeneratedSymbolKind::TlsAccessor)
            .count(),
        2
    );
    let block = module
        .globals
        .iter()
        .find(|global| global.name == "block_value")
        .unwrap();
    let block_manifest = output
        .manifest
        .symbols()
        .iter()
        .find(|symbol| symbol.name == block.emission.symbol_name)
        .unwrap();
    assert_eq!(
        block_manifest.kind,
        ccc_link::bridge::GeneratedSymbolKind::TlsObject
    );
    assert_eq!(
        block_manifest.visibility,
        ccc_link::artifact::GeneratedSymbolVisibility::SourceInternal
    );
    let block_accessor = output
        .assemblies
        .iter()
        .find(|assembly| assembly.source().contains(&block.emission.symbol_name))
        .unwrap();
    assert!(
        block_accessor
            .source()
            .contains(&format!(".hidden {}", block.emission.symbol_name))
    );

    let object = object::File::parse(output.object.as_slice()).unwrap();
    for global in &module.globals {
        let symbol = object
            .symbol_by_name(&global.emission.symbol_name)
            .unwrap_or_else(|| panic!("missing TLS symbol `{}`", global.emission.symbol_name));
        assert_eq!(symbol.kind(), object::SymbolKind::Tls);
    }
    output.into_artifact_bundle().verify().unwrap();
}

#[test]
fn debug_information_distinguishes_prototypes_and_locates_tls_definitions() {
    let config = EffectiveCompilationConfig::default();
    let (module, sources) = lower_source_with_map(
        "_Thread_local int debug_tls = 7;\n\
         int (*unspecified_function_pointer)();\n\
         int prototyped(int value) { return value + debug_tls; }",
        &config,
    );
    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: false,
            debug_info: Some(&sources),
        },
    )
    .unwrap();
    let object = object::File::parse(output.object.as_slice()).unwrap();

    let debug_info = object.section_by_name(".debug_info").unwrap();
    assert!(debug_info.relocations().any(|(_, relocation)| {
        if relocation.flags()
            != (RelocationFlags::Elf {
                r_type: object::elf::R_X86_64_DTPOFF64,
            })
        {
            return false;
        }
        let RelocationTarget::Symbol(index) = relocation.target() else {
            return false;
        };
        object
            .symbol_by_index(index)
            .is_ok_and(|symbol| symbol.name() == Ok("debug_tls"))
    }));

    let sections = gimli::DwarfSections::load(|id| {
        Ok::<_, gimli::Error>(
            object
                .section_by_name(id.name())
                .and_then(|section| section.data().ok())
                .unwrap_or_default()
                .to_vec(),
        )
    })
    .unwrap();
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, gimli::LittleEndian));
    let mut units = dwarf.units();
    let header = units.next().unwrap().expect("debug compilation unit");
    let unit = dwarf.unit(header).unwrap();
    let mut entries = unit.entries();
    let mut prototyped_subprogram = false;
    let mut prototyped_subroutine_type = false;
    let mut unspecified_subroutine_type = false;
    let mut tls_location = false;
    while let Some(entry) = entries.next_dfs().unwrap() {
        let has_prototype = entry.has_attr(gimli::DW_AT_prototyped);
        if entry.tag() == gimli::DW_TAG_subroutine_type {
            prototyped_subroutine_type |= has_prototype;
            unspecified_subroutine_type |= !has_prototype;
        }
        let name = entry
            .attr(gimli::DW_AT_name)
            .and_then(|attribute| dwarf.attr_string(&unit, attribute.value()).ok())
            .map(|name| name.to_string_lossy().into_owned());
        if entry.tag() == gimli::DW_TAG_subprogram && name.as_deref() == Some("prototyped") {
            prototyped_subprogram = has_prototype;
        }
        if entry.tag() == gimli::DW_TAG_variable && name.as_deref() == Some("debug_tls") {
            let gimli::AttributeValue::Exprloc(expression) =
                entry.attr_value(gimli::DW_AT_location).unwrap()
            else {
                panic!("defined TLS object has no debug location");
            };
            let mut operations = expression.operations(unit.encoding());
            assert!(matches!(
                operations.next().unwrap(),
                Some(gimli::Operation::UnsignedConstant { value: 0 })
            ));
            assert!(matches!(
                operations.next().unwrap(),
                Some(gimli::Operation::TLS)
            ));
            assert!(operations.next().unwrap().is_none());
            tls_location = true;
        }
    }
    assert!(prototyped_subprogram);
    assert!(prototyped_subroutine_type);
    assert!(unspecified_subroutine_type);
    assert!(tls_location);
}

#[test]
fn debug_parameters_use_cranelift_value_location_ranges() {
    let config = EffectiveCompilationConfig::default();
    let (module, sources) = lower_source_with_map(
        "static int add(int left, int right) { return left + right; }\n\
         int main(void) { return add(20, 22) == 42 ? 0 : 1; }",
        &config,
    );
    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: false,
            debug_info: Some(&sources),
        },
    )
    .unwrap();
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(object.section_by_name(".debug_loc").is_some());

    let sections = gimli::DwarfSections::load(|id| {
        Ok::<_, gimli::Error>(
            object
                .section_by_name(id.name())
                .and_then(|section| section.data().ok())
                .unwrap_or_default()
                .to_vec(),
        )
    })
    .unwrap();
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, gimli::LittleEndian));
    let mut units = dwarf.units();
    let header = units.next().unwrap().expect("debug compilation unit");
    let unit = dwarf.unit(header).unwrap();
    let mut entries = unit.entries();
    let mut located = BTreeSet::new();
    while let Some(entry) = entries.next_dfs().unwrap() {
        if entry.tag() != gimli::DW_TAG_formal_parameter {
            continue;
        }
        let name = entry
            .attr(gimli::DW_AT_name)
            .and_then(|attribute| dwarf.attr_string(&unit, attribute.value()).ok())
            .map(|name| name.to_string_lossy().into_owned());
        let Some(name @ ("left" | "right")) = name.as_deref() else {
            continue;
        };
        assert!(matches!(
            entry.attr_value(gimli::DW_AT_location).unwrap(),
            gimli::AttributeValue::LocationListsRef(_)
        ));
        located.insert(name.to_owned());
    }
    assert_eq!(
        located,
        BTreeSet::from(["left".to_owned(), "right".to_owned()])
    );
}

#[test]
fn darwin_debug_code_addresses_are_text_section_relative() {
    let config = EffectiveCompilationConfig::aarch64_apple_darwin();
    let (module, sources) = lower_source_with_map(
        "static int first(int value) { return value + 1; }\n\
         static int second(int value) { return value * 2; }\n\
         int main(void) { return second(first(20)) == 42 ? 0 : 1; }",
        &config,
    );
    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: false,
            debug_info: Some(&sources),
        },
    )
    .unwrap();
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let text = object
        .section_by_name("__text")
        .expect("Darwin text section");
    let line = object
        .section_by_name("__debug_line")
        .expect("Darwin line table");
    let line_data = line.data().unwrap();
    let mut sequence_bases = BTreeSet::new();
    let mut text_relocations = 0usize;
    for (offset, relocation) in line.relocations() {
        let targets_text = match relocation.target() {
            RelocationTarget::Section(index) => index == text.index(),
            RelocationTarget::Symbol(index) => {
                let symbol = object.symbol_by_index(index).unwrap();
                symbol.kind() == object::SymbolKind::Section
                    && symbol.section_index() == Some(text.index())
            }
            _ => false,
        };
        if !targets_text {
            continue;
        }
        text_relocations += 1;
        let start = usize::try_from(offset).unwrap();
        let bytes: [u8; 8] = line_data[start..start + 8].try_into().unwrap();
        sequence_bases.insert(u64::from_le_bytes(bytes));
    }
    assert_eq!(text_relocations, 3);
    assert_eq!(sequence_bases.len(), 3);
    assert_eq!(sequence_bases.first().copied(), Some(0));
}

#[test]
fn incomplete_extern_arrays_remain_layout_free_undefined_data() {
    const SOURCE: &str = "extern const char bytes[];\n\
         extern int values[];\n\
         int read_imports(void) { return bytes[0] + values[0]; }";
    let module = lower_source(SOURCE);
    for name in ["bytes", "values"] {
        let global = module
            .globals
            .iter()
            .find(|global| global.name == name)
            .unwrap();
        assert_eq!(
            global.emission.definition,
            ObjectDefinitionPolicy::Declaration
        );
        let ccc_types::TypeKind::Array(array) = module.types.kind(global.ty.ty) else {
            panic!("{name} should have array type")
        };
        assert_eq!(array.length, ccc_types::ArrayLength::Incomplete);
    }

    let config = EffectiveCompilationConfig::default();
    ccc_abi::plan_module(&module, &config).unwrap();
    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    let object = object::File::parse(output.object.as_slice()).unwrap();
    for name in ["bytes", "values"] {
        let symbol = object
            .symbol_by_name(name)
            .unwrap_or_else(|| panic!("missing `{name}`"));
        assert!(symbol.is_undefined(), "{name}");
        assert!(symbol.is_global(), "{name}");
        assert_eq!(symbol.kind(), object::SymbolKind::Data, "{name}");
    }
}

fn symbol_bytes(object_bytes: &[u8], name: &str) -> Vec<u8> {
    let file = object::File::parse(object_bytes).unwrap();
    let symbol = file
        .symbols()
        .find(|symbol| symbol.name() == Ok(name))
        .unwrap_or_else(|| panic!("missing symbol `{name}`"));
    let section = file
        .section_by_index(symbol.section_index().expect("defined symbol"))
        .unwrap();
    let data = section.data().unwrap();
    let start = usize::try_from(symbol.address() - section.address()).unwrap();
    let end = start + usize::try_from(symbol.size()).unwrap();
    data[start..end].to_vec()
}

fn function_clif<'a>(clif: &'a str, name: &str) -> &'a str {
    let marker = format!("; function {name}\n");
    let start = clif
        .find(&marker)
        .unwrap_or_else(|| panic!("missing CLIF function `{name}` in:\n{clif}"));
    let body = &clif[start + marker.len()..];
    let end = body.find("; function ").unwrap_or(body.len());
    &body[..end]
}

#[test]
fn computed_goto_uses_a_dense_br_table_and_nonrelocatable_label_tokens() {
    let output = emit_source(
        "int dispatch(int which) {\n\
             static void *table[2] = {&&left, &&right};\n\
             goto *table[which];\n\
         left: return 11;\n\
         right: return 22;\n\
         }\n\
         int invalid(void) { goto *(void *)0; unused: return 0; }",
    );
    let dispatch = function_clif(&output.clif, "dispatch");
    assert_eq!(dispatch.matches("br_table").count(), 1, "{dispatch}");
    assert!(dispatch.contains("[block0, block1]"), "{dispatch}");
    assert!(dispatch.contains("trap user1"), "{dispatch}");

    let invalid = function_clif(&output.clif, "invalid");
    assert!(invalid.contains("iconst.i32 0"), "{invalid}");
    assert!(
        invalid.contains("iadd_imm") && invalid.contains("-1"),
        "{invalid}"
    );
    assert!(invalid.contains("icmp_imm ugt"), "{invalid}");
    assert!(invalid.contains("br_table"), "{invalid}");
    assert!(invalid.contains("trap user1"), "{invalid}");

    let table_name = "__ccc_block_static.dispatch.0.1.table";
    assert_eq!(
        symbol_bytes(&output.object, table_name),
        [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let table = object.symbol_by_name(table_name).unwrap();
    let section = object
        .section_by_index(table.section_index().unwrap())
        .unwrap();
    assert_eq!(section.relocations().count(), 0);
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn complete_abi_plan_and_aggregate_clif_have_exact_snapshots() {
    const SOURCE: &str = "typedef __builtin_va_list va_list;\n\
         struct Mixed { double floating; long integer; };\n\
         struct Large { long first; long second; long third; };\n\
         short narrow(short value) { return value; }\n\
         struct Mixed native(struct Mixed value) { return value; }\n\
         struct Large native_large(struct Large value) { return value; }\n\
         long collect(int count, ...) {\n\
             va_list list;\n\
             struct Mixed value;\n\
             __builtin_va_start(list, count);\n\
             value = __builtin_va_arg(list, struct Mixed);\n\
             __builtin_va_end(list);\n\
             return value.integer + count;\n\
         }\n\
         long invoke(struct Mixed value) {\n\
             struct Mixed (*indirect)(struct Mixed) = native;\n\
             return indirect(value).integer + collect(1, value);\n\
         }";
    let module = lower_source(SOURCE);
    let config = EffectiveCompilationConfig::default();
    let plan = ccc_abi::plan_module(&module, &config).unwrap();
    let dump = ccc_abi::dump_module_plan(plan.verify_against(&module, &config).unwrap());
    assert!(dump.contains("lowered-signature=native"), "{dump}");
    assert!(dump.contains("extension=signed"), "{dump}");
    assert!(dump.contains("hidden-return=true"), "{dump}");
    assert!(dump.contains("target=direct:"), "{dump}");
    assert!(dump.contains("target=indirect:"), "{dump}");
    assert!(dump.contains("call-bridge helper="), "{dump}");
    assert!(dump.contains("packaging assembly-units=2"), "{dump}");
    assert_eq!(
        sha256(&dump),
        "e473b3371dd953205ecfc1e704bffbaebe63487d238f7c38febab5010a041441"
    );

    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    assert_eq!(
        sha256(&output.clif),
        "66995b11436d037c292c8ea80138de4c73d2ee8ba8f9fdabc5a96daa41d4ebdd"
    );
}

#[test]
fn pinned_cranelift_accepts_rounded_struct_arguments_and_structure_returns() {
    let mut config = EffectiveCompilationConfig::default();
    config.capabilities.insert(
        ccc_target::CapabilityKind::Attribute,
        "packed",
        ccc_target::CapabilityState::Implemented,
    );
    let module = lower_source_with_config(
        "struct __attribute__((packed)) One { char byte; };\n\
         struct __attribute__((packed)) Nine { long word; char byte; };\n\
         struct Seventeen { char bytes[17]; };\n\
         void consume(long a, long b, long c, long d, long e, long f,\n\
                      struct One one, struct Nine nine, struct Seventeen seventeen) {}\n\
         struct Seventeen identity(struct Seventeen value) { return value; }",
        &config,
    );
    let plan = ccc_abi::plan_module(&module, &config).unwrap();
    let consume = module
        .functions
        .iter()
        .find(|function| function.symbol_name == "consume")
        .unwrap();
    let ccc_abi::BoundaryPlan::Native(consume_plan) =
        &plan.definitions.get(&consume.id).unwrap().boundary
    else {
        panic!("fixed definition must use the native boundary")
    };
    let signature = super::super::signature(consume_plan).unwrap();
    assert_eq!(
        signature.params[6].purpose,
        cranelift_codegen::ir::ArgumentPurpose::StructArgument(8)
    );
    assert_eq!(
        signature.params[7].purpose,
        cranelift_codegen::ir::ArgumentPurpose::StructArgument(16)
    );
    assert_eq!(
        signature.params[8].purpose,
        cranelift_codegen::ir::ArgumentPurpose::StructArgument(24)
    );

    let identity = module
        .functions
        .iter()
        .find(|function| function.symbol_name == "identity")
        .unwrap();
    let ccc_abi::BoundaryPlan::Native(identity_plan) =
        &plan.definitions.get(&identity.id).unwrap().boundary
    else {
        panic!("fixed definition must use the native boundary")
    };
    let signature = super::super::signature(identity_plan).unwrap();
    assert_eq!(
        signature.params[0].purpose,
        cranelift_codegen::ir::ArgumentPurpose::StructReturn
    );
    assert_eq!(
        signature.params[1].purpose,
        cranelift_codegen::ir::ArgumentPurpose::StructArgument(24)
    );
    assert!(signature.returns.is_empty());

    emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
}

#[test]
fn string_initializers_respect_exact_bounds_and_zero_fill_remainder() {
    let output = emit_source(
        "char exact[2] = \"xy\";\n\
         char padded[4] = \"xy\";\n\
         int main(void) { return exact[1] + padded[2] + padded[3]; }",
    );
    assert_eq!(symbol_bytes(&output.object, "exact"), b"xy");
    assert_eq!(symbol_bytes(&output.object, "padded"), b"xy\0\0");
}

#[test]
fn nonzero_offset_bitfields_use_their_projected_storage_once() {
    let output = emit_source(
        "struct Bits { unsigned prefix; unsigned value : 5; };\n\
         struct Bits bits = { .prefix = 0x11223344u, .value = 7 };\n\
         unsigned update(struct Bits *value) {\n\
             value->value = 9;\n\
             return value->value;\n\
         }",
    );
    assert_eq!(
        symbol_bytes(&output.object, "bits"),
        [0x44, 0x33, 0x22, 0x11, 0x07, 0x00, 0x00, 0x00]
    );
    let clif = function_clif(&output.clif, "update");
    assert!(clif.contains("iadd_imm"), "{clif}");
}

#[test]
fn aggregate_rvalue_bitfields_lower_through_their_projection_anchor() {
    let output = emit_source(
        "struct Inner { unsigned prefix; signed value : 6; };\n\
         struct Outer { struct Inner inner; unsigned tail; };\n\
         struct Outer make(void);\n\
         int read(void) { return make().inner.value; }",
    );
    let clif = function_clif(&output.clif, "read");
    assert!(clif.contains("band_imm"), "{clif}");
    assert!(clif.contains("sshr_imm"), "{clif}");
}

#[test]
fn repeated_initializers_use_target_stride_and_preserve_relocations() {
    let output = emit_source(
        "int target;\n\
         int repeated[4] = {7, 7, 7};\n\
         int *addresses[3] = {&target, &target, &target};",
    );
    assert_eq!(
        symbol_bytes(&output.object, "repeated"),
        [7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,]
    );
    assert_eq!(symbol_bytes(&output.object, "addresses"), [0; 24]);

    let object = object::File::parse(output.object.as_slice()).unwrap();
    let addresses = object
        .symbols()
        .find(|symbol| symbol.name() == Ok("addresses"))
        .expect("addresses symbol");
    let section = object
        .section_by_index(addresses.section_index().expect("defined addresses"))
        .unwrap();
    let relocations = section
        .relocations()
        .filter_map(|(offset, relocation)| {
            let RelocationTarget::Symbol(symbol) = relocation.target() else {
                return None;
            };
            (object.symbol_by_index(symbol).unwrap().name() == Ok("target"))
                .then_some(offset - addresses.address())
        })
        .collect::<Vec<_>>();
    assert_eq!(relocations, [0, 8, 16]);
}

#[test]
fn runtime_bool_conversion_normalizes_nonzero_values() {
    let output = emit_source("int normalize(int x) { _Bool truth = x; return truth; }");
    let clif = function_clif(&output.clif, "normalize");
    assert!(clif.contains("icmp_imm ne"), "{clif}");
    assert!(!clif.contains("store"), "{clif}");
    assert!(!clif.contains("load.i8"), "{clif}");
}

#[test]
fn integer_to_pointer_widening_preserves_source_signedness() {
    let output = emit_source(
        "void *from_signed(int value) { return (void *)value; }\n\
         void *from_unsigned(unsigned int value) { return (void *)value; }",
    );
    let signed = function_clif(&output.clif, "from_signed");
    assert!(signed.contains("sextend.i64"), "{signed}");
    assert!(!signed.contains("uextend.i64"), "{signed}");

    let unsigned = function_clif(&output.clif, "from_unsigned");
    assert!(unsigned.contains("uextend.i64"), "{unsigned}");
    assert!(!unsigned.contains("sextend.i64"), "{unsigned}");
}

#[test]
fn static_bool_initializers_normalize_and_tentative_data_uses_elf_common() {
    let output = emit_source(
        "static _Bool normalized = 7;\n\
         int tentative;\n\
         int main(void) { return normalized + tentative; }",
    );
    assert_eq!(symbol_bytes(&output.object, "normalized"), [1]);
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let common = object
        .symbols()
        .find(|symbol| symbol.name() == Ok("tentative"))
        .expect("tentative symbol");
    assert_eq!(common.section(), object::SymbolSection::Common);
    assert_eq!(common.size(), 4);
    assert_eq!(common.address(), 4, "ELF common value is its alignment");
}

#[test]
fn unused_unsupported_function_declarations_do_not_poison_emission() {
    let output = emit_source(
        "long double native_width(long double);\n\
         long double (*selected_width)(long double) = native_width;\n\
         int variadic(const char *, ...);\n\
         int main(void) { return 0; }",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(object.symbols().any(|symbol| symbol.name() == Ok("main")));
}

#[test]
fn direct_external_calls_use_branch_relocations_and_function_pointers_stay_indirect() {
    let output = emit_source(
        "int imported(int);\n\
         int direct(int value) { return imported(value); }\n\
         int indirect(int (*callee)(int), int value) { return callee(value); }",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    let relocations = object
        .sections()
        .flat_map(|section| section.relocations())
        .filter_map(|(_, relocation)| {
            let RelocationTarget::Symbol(index) = relocation.target() else {
                return None;
            };
            (object.symbol_by_index(index).unwrap().name() == Ok("imported")).then_some(relocation)
        })
        .collect::<Vec<_>>();
    assert_eq!(relocations.len(), 1, "{relocations:#?}");
    assert_eq!(relocations[0].kind(), RelocationKind::PltRelative);
    assert_eq!(relocations[0].encoding(), RelocationEncoding::X86Branch);
    assert_eq!(relocations[0].size(), 32);

    let direct = function_clif(&output.clif, "direct");
    assert!(direct.contains("call fn"), "{direct}");
    assert!(!direct.contains("call_indirect"), "{direct}");
    let indirect = function_clif(&output.clif, "indirect");
    assert!(indirect.contains("call_indirect"), "{indirect}");
}

#[test]
fn certified_inline_assembly_reaches_native_operations_and_exact_support_helpers() {
    let output = emit_source(
        "void exercise(unsigned *eax_out, unsigned *ebx_out, unsigned *ecx_out, unsigned *edx_out,\n\
                       unsigned long *field, unsigned long *expected, unsigned long desired,\n\
                       unsigned index, unsigned low, unsigned long *backup) {\n\
             unsigned eax, ebx, ecx, edx;\n\
             unsigned long value = desired;\n\
             unsigned long original;\n\
             unsigned long *candidate = field;\n\
             asm(\"\" : \"+r\"(candidate));\n\
             asm volatile(\"\" ::: \"memory\");\n\
             asm(\".p2align 5\");\n\
             asm(\"nop\");\n\
             asm(\"cpuid\" : \"=a\"(eax), \"=b\"(ebx), \"=c\"(ecx) : \"a\"(7), \"c\"(0) : \"edx\");\n\
             asm volatile(\"rdtsc\" : \"=a\"(eax), \"=d\"(edx));\n\
             asm(\"cmp %1, %2\\ncmova %3, %0\\n\" : \"+r\"(candidate) : \"r\"(index), \"r\"(low), \"r\"(backup));\n\
             asm volatile(\"lock; xchgq %0, %1\" : \"+q\"(value), \"+m\"(*field));\n\
             asm volatile(\"lock; cmpxchgq %2, %1\" : \"=a\"(original), \"+m\"(*field) : \"q\"(desired), \"0\"(*expected));\n\
             *eax_out = eax; *ebx_out = ebx; *ecx_out = ecx; *edx_out = edx;\n\
             *expected = original + (unsigned long)candidate + value;\n\
         }",
    );

    assert_eq!(output.assemblies.len(), 1);
    let support = &output.assemblies[0];
    assert_eq!(support.stem(), "inline-asm-support");
    assert!(support.source().contains("cpuid"), "{}", support.source());
    assert!(support.source().contains("rdtsc"), "{}", support.source());
    assert_eq!(support.defined_symbols().len(), 2);
    assert!(
        support
            .defined_symbols()
            .iter()
            .any(|symbol| symbol.starts_with("__ccc_support_cpuid_"))
    );
    assert!(
        support
            .defined_symbols()
            .iter()
            .any(|symbol| symbol.starts_with("__ccc_support_rdtsc_"))
    );
    for symbol in support.defined_symbols() {
        let manifest = output
            .manifest
            .symbols()
            .iter()
            .find(|entry| entry.name == *symbol)
            .expect("inline-assembly helper manifest entry");
        assert_eq!(
            manifest.kind,
            ccc_link::bridge::GeneratedSymbolKind::Support
        );
    }

    let object = object::File::parse(output.object.as_slice()).unwrap();
    for symbol in support.defined_symbols() {
        assert!(
            object.symbol_by_name(symbol).unwrap().is_undefined(),
            "{symbol} must be resolved by generated support assembly"
        );
    }
    let clif = function_clif(&output.clif, "exercise");
    assert!(clif.contains("fence"), "{clif}");
    assert!(clif.contains("atomic_rmw.i64 xchg"), "{clif}");
    assert!(clif.contains("atomic_cas"), "{clif}");
    assert_eq!(clif.matches("call fn").count(), 2, "{clif}");
}

#[test]
fn x87_long_double_definitions_and_variadic_calls_use_generated_bridges() {
    let definition = lower_source("long double identity(long double value) { return value; }");
    let output = emit(
        &definition,
        &EffectiveCompilationConfig::default(),
        Options::default(),
    )
    .unwrap();
    assert_eq!(output.assemblies.len(), 2);
    assert!(
        output
            .assemblies
            .iter()
            .any(|assembly| assembly.source().contains("fldt 256(%rsp)"))
    );
    assert!(
        output
            .assemblies
            .iter()
            .any(|assembly| assembly.source().contains(".Lccc_f80_add:"))
    );

    let call = lower_source(
        "int variadic(const char *, ...);\n\
         int main(void) { return variadic(\"x\", 1); }",
    );
    let output = emit(
        &call,
        &EffectiveCompilationConfig::default(),
        Options::default(),
    )
    .unwrap();
    assert_eq!(output.assemblies.len(), 1);
    assert!(
        output.assemblies[0].source().contains("call *%r11"),
        "{}",
        output.assemblies[0].source()
    );
}

#[test]
fn x87_fixed_variadic_aggregate_indirect_and_volatile_paths_share_verified_support() {
    let module = lower_source(
        "typedef __builtin_va_list va_list;
         struct Box { long double value; unsigned marker; };
         long double fixed(long double value) { return value + 1.0L; }
         long double read(int count, ...) {
             va_list list;
             __builtin_va_start(list, count);
             return __builtin_va_arg(list, long double);
         }
         struct Box aggregate(struct Box value) {
             value.value = value.value * 2.0L;
             return value;
         }
         long double indirect(long double (*function)(long double), long double value) {
             return function(value);
         }
         void volatile_copy(volatile long double *destination,
                            volatile long double *source) {
             *destination = *source;
         }",
    );
    let config = EffectiveCompilationConfig::default();
    let plan = ccc_abi::plan_module(&module, &config).unwrap();
    assert!(
        plan.definitions
            .values()
            .filter(|plan| matches!(plan.boundary, ccc_abi::BoundaryPlan::Bridge(_)))
            .count()
            >= 4
    );
    let x87_va_arg = plan
        .va_args
        .values()
        .find(|plan| plan.classified.ty == TypeId::LONG_DOUBLE)
        .expect("x87 va_arg plan");
    assert_eq!((x87_va_arg.gp_slots, x87_va_arg.sse_slots), (0, 0));
    assert_eq!(
        (x87_va_arg.overflow_align, x87_va_arg.overflow_size),
        (16, 16)
    );

    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    let support = output
        .assemblies
        .iter()
        .find(|assembly| assembly.stem() == "f80-support")
        .expect("one x87 support unit");
    assert!(support.source().contains("faddp %st, %st(1)"));
    assert!(support.source().contains("fmulp %st, %st(1)"));
    assert!(support.source().contains("fucomip %st(1), %st"));
    assert!(support.source().contains("fcomip %st(1), %st"));
    assert!(
        output
            .assemblies
            .iter()
            .any(|assembly| assembly.source().contains("call *%r11")),
        "the indirect f80 call must use the generic boundary helper"
    );
    let volatile = function_clif(&output.clif, "volatile_copy");
    assert_eq!(volatile.matches("call fn").count(), 2, "{volatile}");
}

#[test]
fn x87_wide_integer_conversions_use_exact_operation_sensitive_runtime_helpers() {
    let output = emit_source(
        "long double from_signed(__int128 value) { return (long double)value; }
         long double from_unsigned(unsigned __int128 value) { return (long double)value; }
         __int128 to_signed(long double value) { return (__int128)value; }
         unsigned __int128 to_unsigned(long double value) { return (unsigned __int128)value; }",
    );
    let support = output
        .assemblies
        .iter()
        .find(|assembly| assembly.stem() == "f80-support")
        .expect("x87 support assembly");
    for symbol in ["__floattixf", "__floatuntixf", "__fixxfti", "__fixunsxfti"] {
        assert!(support.source().contains(&format!("call {symbol}@PLT")));
        assert!(
            EffectiveCompilationConfig::default()
                .target
                .abi
                .runtime_helper_manifest()
                .iter()
                .any(|contract| contract.symbol == symbol),
            "{symbol} must be pre-budgeted by the target runtime manifest"
        );
    }
    let object = object::File::parse(output.object.as_slice()).unwrap();
    for symbol in ["__floattixf", "__floatuntixf", "__fixxfti", "__fixunsxfti"] {
        assert!(
            object.symbol_by_name(symbol).is_none(),
            "{symbol} belongs to generated assembly, not the Cranelift object"
        );
    }

    let signed_only =
        emit_source("long double from_signed(__int128 value) { return (long double)value; }");
    let support = signed_only
        .assemblies
        .iter()
        .find(|assembly| assembly.stem() == "f80-support")
        .unwrap();
    assert!(support.source().contains("call __floattixf@PLT"));
    for absent in ["__floatuntixf", "__fixxfti", "__fixunsxfti"] {
        assert!(!support.source().contains(absent), "{absent}");
    }
}

#[test]
fn unprototyped_calls_use_the_generic_bridge_with_the_promoted_actual_plan() {
    let module = lower_source(
        "int legacy();\n\
         int invoke(float floating, signed char narrow) { return legacy(floating, narrow); }",
    );
    let config = EffectiveCompilationConfig::default();
    let plan = ccc_abi::plan_module(&module, &config).unwrap();
    let call = plan.calls.values().next().unwrap();
    assert_eq!(call.promoted_actual_types, [TypeId::DOUBLE, TypeId::INT]);
    assert_eq!(call.fixed_boundary, 0);
    let ccc_abi::BoundaryPlan::Bridge(boundary) = &call.boundary else {
        panic!("unprototyped call must use the assembly bridge")
    };
    assert_eq!(boundary.kind, ccc_abi::BridgeKind::UnprototypedCall);
    assert_eq!(boundary.variadic_sse_count, 1);

    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    assert_eq!(output.assemblies.len(), 1);
    assert!(
        output.assemblies[0].source().contains("call *%r11"),
        "{}",
        output.assemblies[0].source()
    );
    let clif = function_clif(&output.clif, "invoke");
    assert!(clif.contains("fpromote"), "{clif}");
    assert!(clif.contains("call fn"), "{clif}");
}

#[test]
fn floating_storage_and_a_preallocated_label_keep_the_real_entry_first() {
    let output = emit_source(
        "int classify(float value) {\n\
             float copy = value;\n\
             goto done;\n\
         done:\n\
             return copy != 0.0f;\n\
         }",
    );
    let clif = function_clif(&output.clif, "classify");
    assert!(!clif.contains("store"), "{clif}");
    assert!(clif.contains("fcmp ne"), "{clif}");
}

#[test]
fn volatile_scalar_accesses_have_exact_fence_order_in_clif_and_machine_code() {
    let output = emit_source(
        "int touch(volatile int *p) { int before = *p; *p = before + 1; return before; }",
    );
    let clif = function_clif(&output.clif, "touch");
    let lines = clif.lines().map(str::trim).collect::<Vec<_>>();
    let fences = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (*line == "fence").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(fences.len(), 4, "{clif}");
    let volatile_load = lines
        .iter()
        .position(|line| line.contains("load.i32"))
        .expect("volatile load");
    let volatile_store = lines
        .iter()
        .rposition(|line| line.starts_with("store "))
        .expect("volatile store");
    assert!(
        fences[0] < volatile_load && volatile_load < fences[1],
        "{clif}"
    );
    assert!(
        fences[2] < volatile_store && volatile_store < fences[3],
        "{clif}"
    );

    let machine = symbol_bytes(&output.object, "touch");
    let mfence = [0x0f, 0xae, 0xf0];
    assert_eq!(
        machine
            .windows(mfence.len())
            .filter(|bytes| *bytes == mfence)
            .count(),
        4
    );
}

#[test]
fn sync_synchronize_is_one_native_full_fence_without_an_external_symbol() {
    let output = emit_source("void synchronize(void) { __sync_synchronize(); }");
    let clif = function_clif(&output.clif, "synchronize");
    assert_eq!(
        clif.lines().filter(|line| line.trim() == "fence").count(),
        1,
        "{clif}"
    );

    let machine = symbol_bytes(&output.object, "synchronize");
    let mfence = [0x0f, 0xae, 0xf0];
    assert_eq!(
        machine
            .windows(mfence.len())
            .filter(|bytes| *bytes == mfence)
            .count(),
        1
    );

    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(object.symbol_by_name("__sync_synchronize").is_none());
    assert!(
        object
            .symbols()
            .filter(|symbol| symbol.is_undefined())
            .all(|symbol| symbol.name() != Ok("__sync_synchronize"))
    );
}

#[test]
fn legacy_sync_operations_are_native_atomics_without_external_symbols() {
    let output = emit_source(
        "int value;\n\
         void *pointer;\n\
         int protected_side_effect(void);\n\
         int update(int delta) {\n\
             int old = __sync_fetch_and_add(&value, delta);\n\
             int now = __sync_add_and_fetch(&value, delta, protected_side_effect());\n\
             int after = __sync_sub_and_fetch(&value, delta, __sync_synchronize);\n\
             int changed = __sync_bool_compare_and_swap(&value, old, now);\n\
             int seen = __sync_val_compare_and_swap(&value, now, after);\n\
             pointer = __sync_lock_test_and_set(&pointer, (void *)0);\n\
             pointer = __sync_add_and_fetch(&pointer, 1);\n\
             return old + now + after + changed + seen;\n\
         }",
    );
    let clif = function_clif(&output.clif, "update");
    assert_eq!(clif.matches("atomic_rmw").count(), 5, "{clif}");
    assert_eq!(clif.matches("atomic_cas").count(), 2, "{clif}");
    assert!(clif.contains("atomic_rmw.i32 add"), "{clif}");
    assert!(clif.contains("atomic_rmw.i32 sub"), "{clif}");
    assert!(clif.contains("atomic_rmw.i64 xchg"), "{clif}");

    let machine = symbol_bytes(&output.object, "update");
    assert!(
        machine.iter().filter(|byte| **byte == 0xf0).count() >= 6,
        "the integer RMW and compare-exchange operations must use locked x86 instructions"
    );

    let object = object::File::parse(output.object.as_slice()).unwrap();
    for symbol in [
        "__sync_add_and_fetch",
        "__sync_fetch_and_add",
        "__sync_sub_and_fetch",
        "__sync_bool_compare_and_swap",
        "__sync_val_compare_and_swap",
        "__sync_lock_test_and_set",
    ] {
        assert!(
            object
                .symbols()
                .filter(|candidate| candidate.is_undefined())
                .all(|candidate| candidate.name() != Ok(symbol)),
            "unexpected external reference to {symbol}"
        );
    }
    assert!(
        object
            .sections()
            .flat_map(|section| section.relocations())
            .all(|(_, relocation)| {
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    return true;
                };
                object.symbol_by_index(index).unwrap().name() != Ok("protected_side_effect")
            }),
        "the ignored protected operand must not produce a call relocation"
    );
}

#[test]
fn scalar_c11_and_gnu_atomics_are_native_and_weaker_orders_are_strengthened() {
    let output = emit_source(
        "_Atomic int value;\n\
         int load_store(int replacement) {\n\
             int old = __atomic_load_n(&value, 0);\n\
             __atomic_store_n(&value, replacement, 3);\n\
             return old;\n\
         }\n\
         int update(int operand, int *expected) {\n\
             int result = __atomic_add_fetch(&value, operand, 2);\n\
             result ^= __atomic_fetch_and(&value, 255, 4);\n\
             result ^= __atomic_fetch_or(&value, 16, 0);\n\
             result ^= __atomic_fetch_xor(&value, 3, 1);\n\
             result ^= __atomic_exchange_n(&value, result, 5);\n\
             result ^= __atomic_compare_exchange_n(\n\
                 &value, expected, result, 1, 0, 0);\n\
             __atomic_thread_fence(2);\n\
             __atomic_signal_fence(3);\n\
             return result;\n\
         }",
    );

    let load_store = function_clif(&output.clif, "load_store");
    let lines = load_store.lines().map(str::trim).collect::<Vec<_>>();
    assert!(
        lines.windows(3).any(|window| window[0] == "fence"
            && window[1].contains("load.i32")
            && window[2] == "fence"),
        "{load_store}"
    );
    assert!(
        lines.windows(3).any(|window| window[0] == "fence"
            && window[1].starts_with("store ")
            && window[2] == "fence"),
        "{load_store}"
    );

    let update = function_clif(&output.clif, "update");
    for operation in ["add", "and", "or", "xor", "xchg"] {
        assert!(
            update.contains(&format!("atomic_rmw.i32 {operation}")),
            "{update}"
        );
    }
    assert!(update.contains("atomic_cas"), "{update}");
    assert_eq!(
        update.lines().filter(|line| line.trim() == "fence").count(),
        2,
        "{update}"
    );

    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(
        object
            .symbols()
            .filter(|symbol| symbol.is_undefined())
            .filter_map(|symbol| symbol.name().ok())
            .all(|name| !name.starts_with("__atomic_")),
        "native scalar atomics must not introduce compiler-runtime symbols"
    );
}

#[test]
fn non_native_atomic_representations_fail_closed() {
    for source in [
        "_Atomic double value; double read(void) { return value; }",
        "volatile _Atomic long double value; long double read(void) { return value; }",
    ] {
        let module = lower_source(source);
        let error = emit(
            &module,
            &EffectiveCompilationConfig::default(),
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC4011", "{source}: {error}");
    }
}

#[test]
fn pointer_add_and_fetch_derives_the_raw_new_representation_in_clif() {
    let output =
        emit_source("void *advance(void **slot) { return __sync_add_and_fetch(slot, 1); }");
    let clif = function_clif(&output.clif, "advance");
    assert!(clif.contains("atomic_rmw.i64 add"), "{clif}");
    assert!(clif.lines().any(|line| line.contains(" = iadd ")), "{clif}");
}

#[test]
fn integer_intrinsics_are_native_clif_operations_without_external_symbols() {
    let output = emit_source(
        "unsigned long swap(unsigned long value) { return __builtin_bswap64(value); }\n\
         int clz_int(unsigned int value) { return __builtin_clz(value); }\n\
         int clz_long(unsigned long value) { return __builtin_clzl(value); }\n\
         int clz_long_long(unsigned long long value) { return __builtin_clzll(value); }\n\
         int ctz_int(unsigned int value) { return __builtin_ctz(value); }\n\
         int ctz_long_long(unsigned long long value) { return __builtin_ctzll(value); }\n\
         int popcount_int(unsigned int value) { return __builtin_popcount(value); }\n\
         int popcount_long_long(unsigned long long value) { return __builtin_popcountll(value); }",
    );
    for (operation, expected) in [("bswap", 1), ("clz", 3), ("ctz", 2), ("popcnt", 2)] {
        assert_eq!(
            output
                .clif
                .lines()
                .filter(|line| line.contains(&format!(" = {operation} ")))
                .count(),
            expected,
            "{}",
            output.clif
        );
    }

    let object = object::File::parse(output.object.as_slice()).unwrap();
    for symbol in [
        "__builtin_bswap64",
        "__builtin_clz",
        "__builtin_clzl",
        "__builtin_clzll",
        "__builtin_ctz",
        "__builtin_ctzll",
        "__builtin_popcount",
        "__builtin_popcountll",
    ] {
        assert!(
            object
                .symbols()
                .filter(|candidate| candidate.is_undefined())
                .all(|candidate| candidate.name() != Ok(symbol)),
            "unexpected external reference to {symbol}"
        );
    }
}

#[test]
fn memory_builtins_lower_to_target_libcalls() {
    let output = emit_source(
        "void *operations(char *to, char *from, unsigned long count) {\n\
             __builtin_memcpy(to, from, count);\n\
             __builtin_memmove(to + 1, to, count - 1);\n\
             return __builtin_memset(to, 65, count);\n\
         }",
    );
    let clif = function_clif(&output.clif, "operations");
    assert!(clif.contains("call fn"), "{clif}");

    let object = object::File::parse(output.object.as_slice()).unwrap();
    for symbol in ["memcpy", "memmove", "memset"] {
        assert!(
            object
                .symbols()
                .filter(|candidate| candidate.is_undefined())
                .any(|candidate| candidate.name() == Ok(symbol)),
            "missing target libcall reference to {symbol}"
        );
    }
}

#[test]
fn prefetch_evaluates_its_address_once_without_a_faulting_access_or_symbol() {
    let output = emit_source(
        "void *next_address(void);\n\
         int protected_side_effect(void);\n\
         void hints(void) {\n\
             __builtin_prefetch(\n\
                 next_address(),\n\
                 1 ? 0 : protected_side_effect(),\n\
                 1 ? 3 : protected_side_effect());\n\
             __builtin_prefetch((void *)1, 1, 0);\n\
         }",
    );
    let clif = function_clif(&output.clif, "hints");
    assert_eq!(clif.matches("call fn").count(), 1, "{clif}");
    assert!(!clif.contains("load"), "{clif}");
    assert!(!clif.contains("store"), "{clif}");

    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert!(object.symbol_by_name("__builtin_prefetch").is_none());
    assert!(
        object
            .symbol_by_name("next_address")
            .is_some_and(|symbol| symbol.is_undefined())
    );
    assert!(
        object
            .sections()
            .flat_map(|section| section.relocations())
            .all(|(_, relocation)| {
                let RelocationTarget::Symbol(index) = relocation.target() else {
                    return true;
                };
                object.symbol_by_index(index).unwrap().name() != Ok("protected_side_effect")
            }),
        "the accepted constant hints must not produce a call relocation"
    );
}

#[test]
fn discarded_volatile_scalar_and_aggregate_reads_remain_explicit() {
    let scalar = emit_source("volatile int observed; void consume(void) { observed; }");
    let scalar = function_clif(&scalar.clif, "consume");
    let scalar_lines = scalar.lines().map(str::trim).collect::<Vec<_>>();
    assert_eq!(
        scalar_lines
            .iter()
            .filter(|line| line.contains("load.i32"))
            .count(),
        1,
        "{scalar}"
    );
    assert_eq!(
        scalar_lines
            .iter()
            .filter(|line| line.trim() == "fence")
            .count(),
        2,
        "{scalar}"
    );

    let aggregate = emit_source(
        "struct Pair { int left; int right; };\n\
         volatile struct Pair observed;\n\
         void consume(void) { observed; }",
    );
    let aggregate = function_clif(&aggregate.clif, "consume");
    let aggregate_lines = aggregate.lines().map(str::trim).collect::<Vec<_>>();
    let volatile_reads = aggregate_lines
        .windows(3)
        .filter(|lines| lines[0] == "fence" && lines[1].contains("load.i8") && lines[2] == "fence")
        .count();
    assert_eq!(volatile_reads, 8, "{aggregate}");
    assert_eq!(
        aggregate_lines
            .iter()
            .filter(|line| line.trim() == "fence")
            .count(),
        16,
        "{aggregate}"
    );
}

#[test]
fn volatile_aggregate_copy_orders_source_reads_without_overqualifying_writes() {
    let output = emit_source(
        "struct Pair { int left; int right; };\n\
         void copy(struct Pair *destination, const volatile struct Pair *source) {\n\
             *destination = *source;\n\
         }",
    );
    let clif = function_clif(&output.clif, "copy");
    let lines = clif.lines().map(str::trim).collect::<Vec<_>>();
    let volatile_loads = lines
        .windows(3)
        .enumerate()
        .filter_map(|(index, window)| {
            (window[0] == "fence" && window[1].contains("load.i8") && window[2] == "fence")
                .then_some(index + 1)
        })
        .collect::<Vec<_>>();
    let stores = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("store ").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(volatile_loads.len(), 8, "{clif}");
    assert_eq!(
        lines.iter().filter(|line| line.trim() == "fence").count(),
        16,
        "{clif}"
    );
    assert!(
        volatile_loads.last().unwrap() < stores.first().unwrap(),
        "{clif}"
    );
}

#[test]
fn aggregate_copy_loads_complete_source_before_any_destination_write() {
    let output = emit_source(
        "struct Four { char bytes[4]; };\n\
         void copy(struct Four *destination, struct Four *source) { *destination = *source; }",
    );
    let clif = function_clif(&output.clif, "copy");
    let lines = clif.lines().map(str::trim).collect::<Vec<_>>();
    let byte_loads = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.contains("load.i8").then_some(index))
        .collect::<Vec<_>>();
    let first_store = lines
        .iter()
        .position(|line| line.starts_with("store "))
        .expect("snapshot store");
    assert!(byte_loads.iter().any(|load| *load < first_store), "{clif}");
    let final_store = lines
        .iter()
        .rposition(|line| line.starts_with("store "))
        .expect("destination store");
    let last_load = *byte_loads.last().expect("aggregate byte load");
    assert!(last_load < final_store, "{clif}");
}

#[test]
fn packed_and_bitfield_memory_paths_remain_unaligned_and_explicit() {
    let mut config = EffectiveCompilationConfig::default();
    config.capabilities.insert(
        ccc_target::CapabilityKind::Attribute,
        "packed",
        ccc_target::CapabilityState::Implemented,
    );
    let module = lower_source_with_config(
        "struct __attribute__((packed)) Packed { char tag; int value; };\n\
         struct Bits { unsigned low : 3; signed high : 5; };\n\
         int inspect(struct Packed *packed, struct Bits *bits) {\n\
             bits->low = 5;\n\
             return packed->value + bits->high;\n\
         }",
        &config,
    );
    let output = emit(
        &module,
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    );
    let output = output.unwrap();
    let clif = function_clif(&output.clif, "inspect");
    assert!(clif.contains("iadd_imm"), "{clif}");
    assert!(clif.contains("load.i32"), "{clif}");
    assert!(!clif.contains(" aligned"), "{clif}");
    assert!(clif.contains("ushr_imm") || clif.contains("ushr"), "{clif}");
    assert!(clif.contains("band_imm") || clif.contains("band"), "{clif}");
}

#[test]
fn automatic_alignment_requests_reach_effective_stack_addresses() {
    let output = emit_source(
        "int inspect(void) {\n\
             _Alignas(64) int value = 7;\n\
             volatile int *address = &value;\n\
             return *address;\n\
         }",
    );
    let clif = function_clif(&output.clif, "inspect");
    assert!(clif.contains("explicit_slot 67, align = 16"), "{clif}");
    assert!(clif.contains("iconst.i64 -64"), "{clif}");
    assert!(clif.contains("band"), "{clif}");
}

#[test]
fn runtime_sized_storage_uses_checked_arena_growth_and_cleanup() {
    let output = emit_source(
        "int inspect(int rows, int columns) {
             _Alignas(64) int matrix[rows][columns];
             matrix[rows - 1][columns - 1] = 17;
             return matrix[rows - 1][columns - 1];
         }",
    );
    let object = object::File::parse(output.object.as_slice()).unwrap();
    for name in ["realloc", "free"] {
        let symbol = object
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .unwrap_or_else(|| panic!("missing arena provider import `{name}`"));
        assert!(symbol.is_undefined(), "{name}");
    }

    let clif = function_clif(&output.clif, "inspect");
    assert!(clif.contains("umul_overflow"), "{clif}");
    assert!(clif.contains("uadd_overflow"), "{clif}");
    assert!(clif.contains("trapnz"), "{clif}");
    assert!(clif.contains("iconst.i64 -64"), "{clif}");
    assert!(clif.matches("call").count() >= 2, "{clif}");
}
