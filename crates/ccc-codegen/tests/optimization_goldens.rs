use ccc_codegen::{Options, emit};
use ccc_ir::generic::{FullModule, dump_frontend_ir, lower_frontend, optimize_frontend_for_config};
use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::{EffectiveCompilationConfig, OptimizationLevel};

const SOURCE: &str = "int optimized(int value) {
    int first = value + 3;
    int second = value + 3;
    if (1) return first ^ second;
    return 9;
}";

fn optimized_module(source: &str, config: &EffectiveCompilationConfig) -> FullModule {
    let mut sources = SourceMap::new();
    let file = sources.add_file("optimized-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config).unwrap();
    let mut module = lower_frontend(&typed).unwrap();
    optimize_frontend_for_config(&mut module, config).unwrap();
    module
}

#[test]
fn optimized_ir_and_clif_match_committed_goldens() {
    let config =
        EffectiveCompilationConfig::default().with_optimization_level(OptimizationLevel::O2);
    let module = optimized_module(SOURCE, &config);

    assert_eq!(
        dump_frontend_ir(&module),
        include_str!("../../../tests/frontend/goldens/ir-optimized-cleanup.out")
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
        output.clif.trim_end(),
        include_str!("../../../tests/frontend/goldens/clif-optimized-cleanup.out").trim_end()
    );
}

#[test]
fn optimized_dominance_ssa_lowers_in_cfg_order_on_every_target() {
    let source = include_str!("../../../tests/execution/cases/full_control_flow.c");
    for config in [
        EffectiveCompilationConfig::default(),
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        for level in [OptimizationLevel::O1, OptimizationLevel::O2] {
            let config = config.clone().with_optimization_level(level);
            let module = optimized_module(source, &config);
            emit(
                &module,
                &config,
                Options {
                    emit_clif: true,
                    debug_info: None,
                },
            )
            .unwrap_or_else(|error| panic!("{} {level:?}: {error}", config.target.triple));
        }
    }
}
