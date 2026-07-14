use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, RelocationEncoding, RelocationKind,
    RelocationTarget,
};

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
fn unsupported_definition_and_call_boundaries_keep_abi_diagnostics() {
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
    let error = emit(
        &call,
        &EffectiveCompilationConfig::default(),
        Options::default(),
    )
    .unwrap_err();
    assert_eq!(error.code, "CCC3510");
    assert!(error.span.is_some());
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
    assert_eq!(
        aggregate_lines
            .iter()
            .filter(|line| line.contains("load.i8"))
            .count(),
        8,
        "{aggregate}"
    );
    assert_eq!(
        aggregate_lines
            .iter()
            .filter(|line| line.trim() == "fence")
            .count(),
        16,
        "{aggregate}"
    );
    assert!(
        aggregate_lines
            .iter()
            .all(|line| !line.starts_with("store ")),
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
    let loads = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.contains("load.i8").then_some(index))
        .collect::<Vec<_>>();
    let stores = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("store ").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(loads.len(), 8, "{clif}");
    assert_eq!(stores.len(), 8, "{clif}");
    assert_eq!(
        lines.iter().filter(|line| line.trim() == "fence").count(),
        16,
        "{clif}"
    );
    assert!(loads.last().unwrap() < stores.first().unwrap(), "{clif}");
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
    assert_eq!(byte_loads.len(), 4, "{clif}");
    let stores_after_loads = lines[byte_loads[3] + 1..]
        .iter()
        .filter(|line| line.starts_with("store "))
        .count();
    assert_eq!(stores_after_loads, 4, "{clif}");
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
