use ccc_codegen::{CodegenError, Options, Output, emit};
use ccc_ir::generic::{FullModule, lower_frontend, optimize_frontend_for_config, verify_frontend};
use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::{ENABLED_TARGET_SPECS, EffectiveCompilationConfig, OptimizationLevel};
use object::{Object as _, ObjectSymbol as _};

const SMALL_LEAF: &str = "
    static int leaf(int value) { return value + 7; }
    int caller(int value) { return leaf(value) * 3; }
";

fn enabled_targets() -> impl Iterator<Item = EffectiveCompilationConfig> {
    ENABLED_TARGET_SPECS.iter().map(|profile| {
        EffectiveCompilationConfig::for_target(profile.triple.clone())
            .expect("catalogued target has an effective configuration")
    })
}

fn lower(source: &str, config: &EffectiveCompilationConfig) -> (SourceMap, FullModule) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("inlining-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config)
        .unwrap_or_else(|diagnostics| panic!("semantic diagnostics: {diagnostics:#?}"));
    let mut module = lower_frontend(&typed).unwrap();
    verify_frontend(&module).unwrap();
    optimize_frontend_for_config(&mut module, config).unwrap();
    verify_frontend(&module).unwrap();
    (sources, module)
}

fn compile(
    source: &str,
    config: &EffectiveCompilationConfig,
    debug: bool,
) -> Result<Output, CodegenError> {
    let (sources, module) = lower(source, config);
    emit(
        &module,
        config,
        Options {
            emit_clif: true,
            debug_info: debug.then_some(&sources),
        },
    )
}

fn function_clif<'a>(output: &'a Output, name: &str) -> &'a str {
    let marker = format!("; function {name}\n");
    let start = output
        .clif
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {name} in:\n{}", output.clif));
    let body = &output.clif[start + marker.len()..];
    body.find("\n; function ").map_or(body, |end| &body[..end])
}

fn direct_call_count(clif: &str) -> usize {
    clif.lines().filter(|line| line.contains(" call ")).count()
}

#[test]
fn small_internal_leaf_is_heuristic_only_at_o2_and_o3_on_every_target() {
    for target in enabled_targets() {
        for (level, expected_calls) in [
            (OptimizationLevel::O0, 1),
            (OptimizationLevel::O1, 1),
            (OptimizationLevel::O2, 0),
            (OptimizationLevel::O3, 0),
            (OptimizationLevel::Size, 1),
            (OptimizationLevel::SizeMin, 1),
        ] {
            let config = target.clone().with_optimization_level(level);
            let output = compile(SMALL_LEAF, &config, false)
                .unwrap_or_else(|error| panic!("{} {level:?}: {error}", config.target.triple));
            assert_eq!(
                direct_call_count(function_clif(&output, "caller")),
                expected_calls,
                "{} {level:?}:\n{}",
                config.target.triple,
                output.clif
            );
        }
    }
}

#[test]
fn always_inline_and_noinline_are_exact_and_out_of_line_symbols_remain() {
    let source = "
        static __attribute__((always_inline)) int forced(int value) {
            return value + 1;
        }
        static __attribute__((noinline)) int retained(int value) {
            return value + 2;
        }
        int (*forced_address)(int) = forced;
        int caller(int value) { return forced(value) + retained(value); }
    ";
    for target in enabled_targets() {
        for level in [OptimizationLevel::O0, OptimizationLevel::O2] {
            let config = target.clone().with_optimization_level(level);
            let output = compile(source, &config, false).unwrap();
            assert_eq!(direct_call_count(function_clif(&output, "caller")), 1);

            let object = object::File::parse(output.object.as_slice()).unwrap();
            let symbols = object
                .symbols()
                .filter_map(|symbol| symbol.name().ok())
                .collect::<Vec<_>>();
            assert!(
                symbols.iter().any(|symbol| symbol.ends_with("forced")),
                "{}: {symbols:?}",
                config.target.triple
            );
            assert!(
                symbols.iter().any(|symbol| symbol.ends_with("retained")),
                "{}: {symbols:?}",
                config.target.triple
            );
            assert!(
                symbols
                    .iter()
                    .any(|symbol| symbol.ends_with("forced_address")),
                "{}: {symbols:?}",
                config.target.triple
            );
        }
    }
}

#[test]
fn interposable_weak_recursive_returns_twice_and_indirect_calls_stay_out() {
    let source = "
        static int leaf(int value) { return value + 1; }
        int external_leaf(int value) { return value + 2; }
        __attribute__((weak)) int weak_leaf(int value) { return value + 3; }
        static int recursive(int value) {
            return value == 0 ? 0 : recursive(value - 1);
        }
        extern int checkpoint(void *) __attribute__((returns_twice));

        int call_external(int value) { return external_leaf(value); }
        int call_weak(int value) { return weak_leaf(value); }
        int call_recursive(int value) { return recursive(value); }
        int returns_twice_caller(void *state, int value) {
            int observed = checkpoint(state);
            return observed + leaf(value);
        }
        int call_indirect(int (*function)(int), int value) {
            return function(value);
        }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let output = compile(source, &config, false)
            .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
        for function in ["call_external", "call_weak", "call_recursive"] {
            assert_eq!(
                direct_call_count(function_clif(&output, function)),
                1,
                "{} {function}:\n{}",
                config.target.triple,
                output.clif
            );
        }
        assert_eq!(
            direct_call_count(function_clif(&output, "returns_twice_caller")),
            2,
            "{}:\n{}",
            config.target.triple,
            output.clif
        );
        assert!(
            function_clif(&output, "call_indirect").contains("call_indirect"),
            "{}:\n{}",
            config.target.triple,
            output.clif
        );
    }
}

#[test]
fn user_named_global_values_keep_calls_out_on_every_target() {
    let source = "
        static int next(void) {
            static int value = 2;
            value += 3;
            return value;
        }
        int caller(void) {
            int first = next();
            int second = next();
            return first * 10 + second;
        }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let output = compile(source, &config, false)
            .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
        assert_eq!(
            direct_call_count(function_clif(&output, "caller")),
            2,
            "{}:\n{}",
            config.target.triple,
            output.clif
        );
    }
}

#[test]
fn user_named_global_values_reject_required_inlining() {
    let source = "
        static __attribute__((always_inline)) int next(void) {
            static int value = 2;
            value += 3;
            return value;
        }
        int caller(void) { return next(); }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let error = compile(source, &config, false).unwrap_err();
        assert_eq!(error.code, "CCC4012");
        assert_eq!(
            error.message,
            "cannot honor `always_inline` call to `next`: the definition references user-named global storage"
        );
    }
}

#[test]
fn debug_builds_keep_heuristic_calls_and_reject_required_inlining() {
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let output = compile(SMALL_LEAF, &config, true)
            .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
        assert_eq!(direct_call_count(function_clif(&output, "caller")), 1);

        let error = compile(
            "
                static __attribute__((always_inline)) int leaf(int value) {
                    return value + 1;
                }
                int caller(int value) { return leaf(value); }
            ",
            &config,
            true,
        )
        .unwrap_err();
        assert_eq!(error.code, "CCC4012");
        assert!(
            error.message.contains("inline debug information"),
            "{}: {error}",
            config.target.triple
        );
    }
}

#[test]
fn unsafe_always_inline_has_a_stable_diagnostic() {
    let source = "
        __attribute__((always_inline)) int exported(int value) {
            return value + 1;
        }
        int caller(int value) { return exported(value); }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let error = compile(source, &config, false).unwrap_err();
        assert_eq!(error.code, "CCC4012");
        assert_eq!(
            error.message,
            "cannot honor `always_inline` call to `exported`: the definition is weak or has external linkage"
        );
        assert!(error.span.is_some());
    }
}

#[test]
fn always_inline_overrides_the_heuristic_size_budget() {
    let source = "
        static __attribute__((always_inline)) int large(int value) {
            value += 1; value += 2; value += 3; value += 4;
            value += 5; value += 6; value += 7; value += 8;
            value += 9; value += 10; value += 11; value += 12;
            value += 13; value += 14; value += 15; value += 16;
            value += 17; value += 18; value += 19; value += 20;
            value += 21; value += 22; value += 23; value += 24;
            value += 25; value += 26; value += 27; value += 28;
            value += 29; value += 30; value += 31; value += 32;
            value += 33; value += 34; value += 35; value += 36;
            return value;
        }
        int caller(int value) { return large(value); }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O0);
        let output = compile(source, &config, false).unwrap();
        assert!(
            function_clif(&output, "large").lines().count() > 32,
            "{}:\n{}",
            config.target.triple,
            output.clif
        );
        assert_eq!(
            direct_call_count(function_clif(&output, "caller")),
            0,
            "{}:\n{}",
            config.target.triple,
            output.clif
        );
    }
}

#[test]
fn bridge_callers_are_not_rewritten() {
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let output = compile(
            "
                static int leaf(int value) { return value + 1; }
                int variadic(int marker, ...) { return leaf(marker); }
            ",
            &config,
            false,
        )
        .unwrap();
        assert_eq!(
            direct_call_count(function_clif(&output, "variadic")),
            1,
            "{}",
            output.clif
        );
    }
}

#[test]
fn per_caller_site_budget_has_an_exact_deterministic_boundary() {
    let source = "
        static int leaf(int value) { return value + 1; }
        int eight(int value) {
            return leaf(value) + leaf(value) + leaf(value) + leaf(value)
                + leaf(value) + leaf(value) + leaf(value) + leaf(value);
        }
        int nine(int value) {
            return leaf(value) + leaf(value) + leaf(value) + leaf(value)
                + leaf(value) + leaf(value) + leaf(value) + leaf(value)
                + leaf(value);
        }
    ";
    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let first = compile(source, &config, false).unwrap();
        let second = compile(source, &config, false).unwrap();
        assert_eq!(first.clif, second.clif, "{}", config.target.triple);
        assert_eq!(first.object, second.object, "{}", config.target.triple);
        assert_eq!(
            direct_call_count(function_clif(&first, "eight")),
            0,
            "{}:\n{}",
            config.target.triple,
            first.clif
        );
        assert_eq!(
            direct_call_count(function_clif(&first, "nine")),
            1,
            "{}:\n{}",
            config.target.triple,
            first.clif
        );
    }
}

#[test]
fn translation_unit_budget_has_an_exact_boundary_and_required_calls_override_it() {
    let mut source = String::from(
        "
        static int leaf(int value) { return value + 1; }
        static __attribute__((always_inline)) int forced(int value) {
            return value + 2;
        }
        ",
    );
    for caller in 0..65 {
        source.push_str(&format!(
            "int caller_{caller}(int value) {{ return leaf(value); }}\n"
        ));
    }
    source.push_str("int forced_caller(int value) { return forced(value); }\n");

    for target in enabled_targets() {
        let config = target.with_optimization_level(OptimizationLevel::O2);
        let first = compile(&source, &config, false).unwrap();
        let second = compile(&source, &config, false).unwrap();
        assert_eq!(first.clif, second.clif, "{}", config.target.triple);
        assert_eq!(first.object, second.object, "{}", config.target.triple);
        assert_eq!(
            direct_call_count(function_clif(&first, "caller_63")),
            0,
            "{}:\n{}",
            config.target.triple,
            first.clif
        );
        assert_eq!(
            direct_call_count(function_clif(&first, "caller_64")),
            1,
            "{}:\n{}",
            config.target.triple,
            first.clif
        );
        assert_eq!(
            direct_call_count(function_clif(&first, "forced_caller")),
            0,
            "{}:\n{}",
            config.target.triple,
            first.clif
        );
    }
}
