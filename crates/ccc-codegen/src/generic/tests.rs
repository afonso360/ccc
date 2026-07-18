use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, RelocationEncoding, RelocationKind,
    RelocationTarget,
};
use sha2::{Digest as _, Sha256};

use super::*;

fn lower_source_with_config(source: &str, config: &EffectiveCompilationConfig) -> gir::FullModule {
    let mut sources = SourceMap::new();
    let file = sources.add_file("generic-codegen-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config)
        .unwrap_or_else(|diagnostics| panic!("semantic diagnostics: {diagnostics:#?}"));
    let module = gir::lower_frontend(&typed).unwrap();
    gir::verify_frontend(&module).unwrap();
    module
}

fn lower_source(source: &str) -> gir::FullModule {
    lower_source_with_config(source, &EffectiveCompilationConfig::default())
}

fn emit_source(source: &str) -> Output {
    emit(
        &lower_source(source),
        &EffectiveCompilationConfig::default(),
        Options { emit_clif: true },
    )
    .unwrap()
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
        Options { emit_clif: true },
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
    let output = emit(&module, &config, Options { emit_clif: true }).unwrap();
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
        "6e96880231371cdc038ca2453549b5d3a0c047b96766193efe448cd1df17c742"
    );

    let output = emit(&module, &config, Options { emit_clif: true }).unwrap();
    assert_eq!(
        sha256(&output.clif),
        "8625817673e96ef125db80b4105b803e1cd4768dce676bacdf5f8a3c23118113"
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

    emit(&module, &config, Options { emit_clif: true }).unwrap();
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
fn long_double_definitions_are_rejected_while_variadic_calls_use_a_bridge() {
    let mut definition = lower_source("long double identity(long double value);");
    definition.functions[0].entry = Some(gir::BlockId(0));
    let error = emit(
        &definition,
        &EffectiveCompilationConfig::default(),
        Options::default(),
    )
    .unwrap_err();
    assert_eq!(error.code, "CCC3509");
    assert!(error.span.is_some());

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

    let output = emit(&module, &config, Options { emit_clif: true }).unwrap();
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
    let output = emit(&module, &config, Options { emit_clif: true });
    let output = output.unwrap();
    let clif = function_clif(&output.clif, "inspect");
    assert!(clif.contains("iadd_imm"), "{clif}");
    assert!(clif.contains("load.i32"), "{clif}");
    assert!(!clif.contains(" aligned"), "{clif}");
    assert!(clif.contains("ushr_imm") || clif.contains("ushr"), "{clif}");
    assert!(clif.contains("band_imm") || clif.contains("band"), "{clif}");
}

#[test]
fn automatic_alignment_requests_reach_cranelift_stack_slots() {
    let output = emit_source(
        "int inspect(void) {\n\
             _Alignas(64) int value = 7;\n\
             volatile int *address = &value;\n\
             return *address;\n\
         }",
    );
    let clif = function_clif(&output.clif, "inspect");
    assert!(clif.contains("explicit_slot 4, align = 64"), "{clif}");
}
