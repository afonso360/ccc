use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::{EffectiveCompilationConfig, OptimizationLevel, enabled_compilation_configs};

use super::super::super::{
    BinaryOperation, FullInstructionKind, FullModule, dump_frontend_ir, lower_frontend,
    verify_frontend,
};
use super::{optimize_frontend_for_config, terminator_edges};

fn lower_source(source: &str, config: &EffectiveCompilationConfig) -> FullModule {
    let mut sources = SourceMap::new();
    let file = sources.add_file("optimizer-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    let typed = analyze_frontend(&parsed, config).unwrap();
    lower_frontend(&typed).unwrap()
}

fn binary_count(module: &FullModule, operator: BinaryOperation) -> usize {
    module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .filter(|instruction| {
            matches!(
                instruction.kind,
                FullInstructionKind::Binary {
                    operator: candidate,
                    ..
                } if candidate == operator
            )
        })
        .count()
}

#[test]
fn local_cse_is_reserved_for_the_higher_and_size_profiles() {
    let source = "int repeat(int x) {
                      int first = x + 3;
                      int second = x + 3;
                      return first ^ second;
                  }";
    let baseline = EffectiveCompilationConfig::default();
    let module = lower_source(source, &baseline);

    let mut o1 = module.clone();
    let o1_config = baseline
        .clone()
        .with_optimization_level(OptimizationLevel::O1);
    optimize_frontend_for_config(&mut o1, &o1_config).unwrap();

    let mut o2 = module;
    let o2_config = baseline.with_optimization_level(OptimizationLevel::O2);
    optimize_frontend_for_config(&mut o2, &o2_config).unwrap();

    assert!(
        binary_count(&o2, BinaryOperation::Add) < binary_count(&o1, BinaryOperation::Add),
        "O1:\n{}\nO2:\n{}",
        dump_frontend_ir(&o1),
        dump_frontend_ir(&o2)
    );
    verify_frontend(&o1).unwrap();
    verify_frontend(&o2).unwrap();
}

#[test]
fn empty_direct_branch_blocks_are_forwarded_but_indirect_targets_are_retained() {
    let config =
        EffectiveCompilationConfig::default().with_optimization_level(OptimizationLevel::O2);
    let mut module = lower_source(
        "int choose(int condition, int value) {
             if (condition) goto finished;
             value += 1;
         finished:
             return value;
         }",
        &config,
    );
    optimize_frontend_for_config(&mut module, &config).unwrap();

    for function in &module.functions {
        let indirect_targets = function
            .blocks
            .iter()
            .filter_map(|block| block.terminator.as_ref())
            .filter_map(|terminator| match terminator {
                super::super::super::FullTerminator::IndirectBranch { targets, .. } => {
                    Some(targets)
                }
                _ => None,
            })
            .flatten()
            .map(|edge| edge.target)
            .collect::<std::collections::BTreeSet<_>>();
        for block in &function.blocks {
            if Some(block.id) == function.entry
                || !block.instructions.is_empty()
                || indirect_targets.contains(&block.id)
            {
                continue;
            }
            if let Some(super::super::super::FullTerminator::Branch(edge)) = &block.terminator {
                assert_eq!(edge.target, block.id, "{}", dump_frontend_ir(&module));
            }
        }
    }
    verify_frontend(&module).unwrap();
}

#[test]
fn every_enabled_profile_reaches_a_stable_verified_form() {
    let source = "int settle(int x) {
                      int duplicate = x + 1;
                      if (1) return duplicate + (x + 1);
                      return 0;
                  }";
    for base in enabled_compilation_configs() {
        for level in [
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
            OptimizationLevel::Size,
            OptimizationLevel::SizeMin,
        ] {
            let config = base.clone().with_optimization_level(level);
            let mut module = lower_source(source, &config);
            optimize_frontend_for_config(&mut module, &config).unwrap();
            let once = dump_frontend_ir(&module);
            optimize_frontend_for_config(&mut module, &config).unwrap();
            assert_eq!(
                dump_frontend_ir(&module),
                once,
                "target {} profile {}",
                config.target.triple,
                level.flag()
            );
            verify_frontend(&module).unwrap();

            // Walking every edge also ensures the retained block-ID space is
            // dense enough for verifier-indexed consumers.
            for function in &module.functions {
                for block in &function.blocks {
                    if let Some(terminator) = &block.terminator {
                        for edge in terminator_edges(terminator) {
                            assert!((edge.target.0 as usize) < function.blocks.len());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn empty_blocks_that_define_live_phi_values_are_not_forwarded() {
    let source = "int preserve(void) {
                      int *saved = 0;
                      for (int index = 0; index < 2; ++index) {
                          int *current = &(int){index + 4};
                          if (index == 0) {
                              saved = current;
                          } else if (current != saved || *current != 5) {
                              return 1;
                          }
                      }
                      return *saved;
                  }";
    for level in [
        OptimizationLevel::O1,
        OptimizationLevel::O2,
        OptimizationLevel::SizeMin,
    ] {
        let config = EffectiveCompilationConfig::default().with_optimization_level(level);
        let mut module = lower_source(source, &config);
        optimize_frontend_for_config(&mut module, &config)
            .unwrap_or_else(|error| panic!("{}: {error:?}", level.flag()));
        verify_frontend(&module).unwrap();
    }
}

#[test]
fn negative_switch_constants_match_at_the_selector_width_on_every_target() {
    let source = "int choose(void) {
                      switch (-1) {
                          case -1: return 1;
                          default: return 2;
                      }
                  }";
    for base in enabled_compilation_configs() {
        for level in [
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::SizeMin,
        ] {
            let config = base.clone().with_optimization_level(level);
            let mut module = lower_source(source, &config);
            optimize_frontend_for_config(&mut module, &config).unwrap();
            let dump = dump_frontend_ir(&module);
            assert!(!dump.contains("switch "), "{dump}");
            assert!(dump.contains("signed:1"), "{dump}");
            assert!(!dump.contains("signed:2"), "{dump}");
            verify_frontend(&module).unwrap();
        }
    }
}

#[test]
fn dead_resultless_void_conversion_releases_its_pure_operand_chain() {
    let config =
        EffectiveCompilationConfig::default().with_optimization_level(OptimizationLevel::O1);
    let mut module = lower_source(
        "int discard(int value) { (value + 1); return value; }",
        &config,
    );

    assert_eq!(binary_count(&module, BinaryOperation::Add), 1);
    optimize_frontend_for_config(&mut module, &config).unwrap();

    assert_eq!(binary_count(&module, BinaryOperation::Add), 0);
    assert!(module.functions[0].blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.kind,
                FullInstructionKind::Convert {
                    kind: super::super::super::ScalarConversion::ToVoid,
                    ..
                }
            )
        })
    }));
    verify_frontend(&module).unwrap();
}

#[test]
fn dead_loop_carried_ssa_cycles_are_removed_by_liveness() {
    let config =
        EffectiveCompilationConfig::default().with_optimization_level(OptimizationLevel::O1);
    let mut module = lower_source(
        "extern int tick(void);\n\
         int discard_cycle(void) {\n\
             unsigned dead = 0;\n\
             while (tick()) ++dead;\n\
             return 0;\n\
         }",
        &config,
    );

    assert_eq!(binary_count(&module, BinaryOperation::Add), 1);
    optimize_frontend_for_config(&mut module, &config).unwrap();

    assert_eq!(binary_count(&module, BinaryOperation::Add), 0);
    assert!(module.functions[1].blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.kind, FullInstructionKind::DirectCall { .. }))
    }));
    verify_frontend(&module).unwrap();
}
