use std::collections::BTreeSet;

use ccc_codegen::{CODEGEN_STATS_SCHEMA_VERSION, Options, Output, emit, emit_with_stats};
use ccc_ir::generic::{FullModule, lower_frontend, optimize_frontend_for_config, verify_frontend};
use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::{ENABLED_TARGET_SPECS, EffectiveCompilationConfig, OptimizationLevel};
use object::{Object as _, ObjectSection as _};

const SOURCE: &str = "
    extern int external(int);
    int global = 7;
    static int leaf(int value) { return value + 7; }
    int entry(int value) { return external(leaf(value)) + global; }
";

fn lower(config: &EffectiveCompilationConfig) -> (SourceMap, FullModule) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("codegen-stats-test.c", SOURCE);
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

fn compile(config: &EffectiveCompilationConfig, debug: bool) -> Output {
    let (sources, module) = lower(config);
    emit_with_stats(
        &module,
        config,
        Options {
            emit_clif: true,
            debug_info: debug.then_some(&sources),
        },
    )
    .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple))
}

#[test]
fn ordinary_emission_does_not_collect_discarded_statistics() {
    let config = EffectiveCompilationConfig::default();
    let (_, module) = lower(&config);
    let output = emit(&module, &config, Options::default()).unwrap();
    assert!(output.stats.is_none());
}

#[test]
fn debug_boundaries_do_not_change_non_debug_clif_or_stats_collection() {
    for profile in ENABLED_TARGET_SPECS {
        let config = EffectiveCompilationConfig::for_target(profile.triple.clone())
            .unwrap()
            .with_optimization_level(OptimizationLevel::O2);
        let (sources, module) = lower(&config);
        let ordinary = emit(
            &module,
            &config,
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap();
        let measured = emit_with_stats(
            &module,
            &config,
            Options {
                emit_clif: true,
                debug_info: None,
            },
        )
        .unwrap();
        let debug = emit(
            &module,
            &config,
            Options {
                emit_clif: true,
                debug_info: Some(&sources),
            },
        )
        .unwrap();

        assert_eq!(ordinary.clif, measured.clif, "{}", profile.triple);
        assert!(
            !ordinary.clif.contains("sequence_point"),
            "{}",
            profile.triple
        );
        assert!(debug.clif.contains("sequence_point"), "{}", profile.triple);
        assert!(ordinary.stats.is_none());
        assert!(measured.stats.is_some());
    }
}

#[test]
fn stats_describe_post_inline_ir_and_primary_objects_on_every_target() {
    for profile in ENABLED_TARGET_SPECS {
        let target = EffectiveCompilationConfig::for_target(profile.triple.clone()).unwrap();
        let o0 = compile(
            &target
                .clone()
                .with_optimization_level(OptimizationLevel::O0),
            true,
        );
        let o2 = compile(
            &target.with_optimization_level(OptimizationLevel::O2),
            false,
        );
        let o0_stats = o0.stats.expect("statistics were requested");
        let o2_stats = o2.stats.expect("statistics were requested");

        assert_eq!(o0_stats.post_inline_ir.functions, 2, "{}", profile.triple);
        assert_eq!(o2_stats.post_inline_ir.functions, 2, "{}", profile.triple);
        assert_eq!(
            o0_stats.post_inline_ir.call_instructions, 2,
            "{}",
            profile.triple
        );
        assert_eq!(
            o2_stats.post_inline_ir.call_instructions, 1,
            "{}",
            profile.triple
        );
        assert!(
            o0_stats.post_inline_ir.values > o0_stats.post_inline_ir.call_instructions,
            "{}: {:?}",
            profile.triple,
            o0_stats.post_inline_ir
        );
        assert!(
            o2_stats.post_inline_ir.values > o2_stats.post_inline_ir.call_instructions,
            "{}: {:?}",
            profile.triple,
            o2_stats.post_inline_ir
        );
        assert!(
            o2_stats.post_inline_ir.instructions > 0,
            "{}: {:?}",
            profile.triple,
            o2_stats.post_inline_ir
        );
        for stats in [o0_stats.post_inline_ir, o2_stats.post_inline_ir] {
            assert!(
                stats.unused_signatures <= stats.signatures,
                "{}: {stats:?}",
                profile.triple
            );
            assert!(
                stats.unused_external_functions <= stats.external_functions,
                "{}: {stats:?}",
                profile.triple
            );
            assert!(
                stats.unused_global_values <= stats.global_values,
                "{}: {stats:?}",
                profile.triple
            );
        }

        assert_primary_object_stats(&o0, &profile.triple.to_string());
        assert_primary_object_stats(&o2, &profile.triple.to_string());
        assert!(
            o0_stats.primary_object.debug_bytes > 0,
            "{}: {:?}",
            profile.triple,
            o0_stats.primary_object
        );
        assert_tsv_schema(&o2);
    }
}

fn assert_primary_object_stats(output: &Output, target: &str) {
    let stats = output
        .stats
        .expect("statistics were requested")
        .primary_object;
    let object = object::File::parse(output.object.as_slice()).unwrap();
    assert_eq!(stats.file_bytes, output.object.len() as u64, "{target}");
    assert_eq!(stats.sections, object.sections().count() as u64, "{target}");
    assert_eq!(
        stats.relocations,
        object
            .sections()
            .map(|section| section.relocations().count() as u64)
            .sum::<u64>(),
        "{target}"
    );
    assert_eq!(
        stats.section_bytes(),
        object.sections().map(|section| section.size()).sum::<u64>(),
        "{target}"
    );
    assert_eq!(
        stats.symbols,
        stats.defined_symbols + stats.undefined_symbols,
        "{target}"
    );
    assert!(stats.text_bytes > 0, "{target}: {stats:?}");
    assert!(stats.unwind_bytes > 0, "{target}: {stats:?}");
}

fn assert_tsv_schema(output: &Output) {
    let stats = output.stats.expect("statistics were requested");
    let tsv = stats.to_tsv();
    let mut keys = BTreeSet::new();
    for line in tsv.lines() {
        let (key, value) = line.split_once('\t').unwrap();
        assert!(keys.insert(key));
        value.parse::<u64>().unwrap();
    }
    let schema_row = format!("schema_version\t{CODEGEN_STATS_SCHEMA_VERSION}");
    assert_eq!(tsv.lines().next(), Some(schema_row.as_str()));
    assert_eq!(keys.len(), stats.metrics().len() + 1);
}
