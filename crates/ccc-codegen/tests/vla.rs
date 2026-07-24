use ccc_codegen::{Options, emit};
use ccc_ir::generic as gir;
use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::{EffectiveCompilationConfig, enabled_compilation_configs};
use object::{Object as _, ObjectSymbol as _};

fn lower(source: &str, config: &EffectiveCompilationConfig) -> gir::FullModule {
    let mut sources = SourceMap::new();
    let file = sources.add_file("vla-codegen-test.c", source);
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
fn runtime_layout_and_storage_emit_for_every_target() {
    let source = "
        unsigned long inspect(int rows, int columns, void *address) {
            typedef int Row[columns++];
            Row matrix[rows];
            matrix[rows - 1][columns - 2] = 17;
            unsigned long cast_size = sizeof *(int (*)[columns++])address;
            return sizeof(matrix) + sizeof(Row) + cast_size + matrix[rows - 1][columns - 2];
        }
    ";
    for config in enabled_compilation_configs() {
        let output = emit(
            &lower(source, &config),
            &config,
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
        let object = object::File::parse(output.object.as_slice()).unwrap();
        let imports = object
            .symbols()
            .filter(|symbol| symbol.is_undefined())
            .filter_map(|symbol| symbol.name().ok())
            .collect::<Vec<_>>();
        assert!(
            imports.iter().any(|name| name.ends_with("realloc")),
            "{}: {imports:?}",
            config.target.triple
        );
        assert!(
            imports.iter().any(|name| name.ends_with("free")),
            "{}: {imports:?}",
            config.target.triple
        );
    }
}

#[test]
fn wide_extents_are_checked_before_size_t_narrowing() {
    let config = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
    let output = emit(
        &lower(
            "unsigned long inspect(unsigned __int128 extent) {
                 int values[extent];
                 return sizeof(values);
             }",
            &config,
        ),
        &config,
        Options {
            emit_clif: true,
            debug_info: None,
        },
    )
    .unwrap();
    assert!(output.clif.contains("ireduce.i64"), "{}", output.clif);
    assert!(output.clif.contains("uextend.i128"), "{}", output.clif);
    assert!(output.clif.contains("icmp ne"), "{}", output.clif);
    assert!(
        output.clif.matches("trapnz").count() >= 2,
        "{}",
        output.clif
    );
}

#[test]
fn runtime_sizeof_without_storage_needs_no_allocator_provider() {
    let source = "
        unsigned long inspect(int extent) {
            typedef int Vector[extent++];
            return sizeof(Vector) + extent;
        }
    ";
    for config in enabled_compilation_configs() {
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
        let imports = object
            .symbols()
            .filter(|symbol| symbol.is_undefined())
            .filter_map(|symbol| symbol.name().ok())
            .collect::<Vec<_>>();
        assert!(
            !imports
                .iter()
                .any(|name| name.ends_with("realloc") || name.ends_with("free")),
            "{}: {imports:?}",
            config.target.triple
        );
    }
}
