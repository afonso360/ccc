use ccc_codegen::{Options, emit};
use ccc_ir::generic as gir;
use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::EffectiveCompilationConfig;
use object::{Object as _, ObjectSection as _, ObjectSymbol as _};

fn lower(source: &str, config: &EffectiveCompilationConfig) -> gir::FullModule {
    let mut sources = SourceMap::new();
    let file = sources.add_file("float16-codegen-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config)
        .unwrap_or_else(|diagnostics| panic!("semantic diagnostics: {diagnostics:#?}"));
    let module = gir::lower_frontend(&typed).unwrap();
    gir::verify_frontend(&module).unwrap();
    module
}

#[test]
fn values_arithmetic_conversions_calls_and_varargs_emit_for_every_target() {
    let source = "
        _Float16 initialized = 1.5;
        _Float16 calculate(_Float16 left, _Float16 right) {
            return left * right + left / right - right;
        }
        double extend(_Float16 value) { return value; }
        _Float16 narrow(double value) { return value; }
        struct HalfPair { _Float16 first; _Float16 second; };
        struct HalfPair exchange(struct HalfPair value) {
            _Float16 temporary = value.first;
            value.first = value.second;
            value.second = temporary;
            return value;
        }
        typedef __builtin_va_list va_list;
        int read_half(int count, ...) {
            va_list arguments;
            __builtin_va_start(arguments, count);
            return __builtin_va_arg(arguments, _Float16) != 0;
        }
        int call_read_half(void) { return read_half(1, (_Float16)1.5); }
    ";
    for config in [
        EffectiveCompilationConfig::default(),
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        emit(
            &lower(source, &config),
            &config,
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
    }
}

#[test]
fn static_initializers_use_exact_binary16_payloads() {
    let source = "
        _Float16 one = 1.0;
        _Float16 negative = -2.0;
        _Float16 tie_down = 1.00048828125;
        _Float16 tie_up = 1.00146484375;
        _Float16 minimum_subnormal = 0x1p-24;
    ";
    for config in [
        EffectiveCompilationConfig::default(),
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        let output = emit(
            &lower(source, &config),
            &config,
            Options {
                emit_clif: false,
                debug_info: None,
            },
        )
        .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
        let object = object::File::parse(output.object.as_slice()).unwrap();
        for (name, expected) in [
            ("one", 0x3c00_u16),
            ("negative", 0xc000),
            ("tie_down", 0x3c00),
            ("tie_up", 0x3c02),
            ("minimum_subnormal", 0x0001),
        ] {
            let symbol_name = if object.format() == object::BinaryFormat::MachO {
                format!("_{name}")
            } else {
                name.to_owned()
            };
            let symbol = object.symbol_by_name(&symbol_name).unwrap();
            let section = object
                .section_by_index(symbol.section_index().unwrap())
                .unwrap();
            let offset = (symbol.address() - section.address()) as usize;
            assert_eq!(
                &section.data().unwrap()[offset..offset + 2],
                &expected.to_le_bytes(),
                "{}: {name}",
                config.target.triple
            );
        }
    }
}

#[test]
fn x87_extended_values_narrow_directly_to_binary16() {
    let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
    let source = "
        _Float16 narrow_extended(long double value) { return value; }
        long double extend_half(_Float16 value) { return value; }
    ";
    emit(
        &lower(source, &config),
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
}
