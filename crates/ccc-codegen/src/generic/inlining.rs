//! Conservative whole-translation-unit policy for Cranelift's inliner.
//!
//! Cranelift owns the transformation. This module supplies only the
//! translation-unit knowledge and deterministic budgets that Cranelift
//! intentionally leaves to its embedding compiler.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use ccc_ir::generic as gir;
use ccc_sema::generic::{Linkage, SymbolBinding};
use ccc_target::OptimizationLevel;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir::{self, ExternalName, Opcode};

use super::{CodegenError, Declarations, INLINE_ERROR, PreparedFunction};

/// Initial size policy. These are raw, frontend-finalized CLIF counts: no CCC
/// clone of Cranelift's simplification or machine optimization is involved.
const MAX_HEURISTIC_CALLEE_INSTRUCTIONS: usize = 24;
const MAX_INLINE_HINT_CALLEE_INSTRUCTIONS: usize = 32;
const MAX_HEURISTIC_CALLEE_BLOCKS: usize = 4;
const MAX_HEURISTIC_SITES_PER_CALLER: usize = 8;
const MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_CALLER: usize = 96;
const MAX_HEURISTIC_BLOCK_GROWTH_PER_CALLER: usize = 16;
// Permit at most eight fully budgeted callers before a translation unit stops
// selecting optional sites. This is a safety ceiling, not profile tuning.
const MAX_HEURISTIC_SITES_PER_TRANSLATION_UNIT: usize = 64;
const MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_TRANSLATION_UNIT: usize = 768;
const MAX_HEURISTIC_BLOCK_GROWTH_PER_TRANSLATION_UNIT: usize = 128;

const CALLER_LIMITS: GrowthLimits = GrowthLimits {
    sites: MAX_HEURISTIC_SITES_PER_CALLER,
    instructions: MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_CALLER,
    blocks: MAX_HEURISTIC_BLOCK_GROWTH_PER_CALLER,
};
const TRANSLATION_UNIT_LIMITS: GrowthLimits = GrowthLimits {
    sites: MAX_HEURISTIC_SITES_PER_TRANSLATION_UNIT,
    instructions: MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_TRANSLATION_UNIT,
    blocks: MAX_HEURISTIC_BLOCK_GROWTH_PER_TRANSLATION_UNIT,
};

pub(super) struct InliningPlan {
    functions: BTreeMap<u32, FunctionInfo>,
    object_to_source: BTreeMap<u32, u32>,
    heuristic_enabled: bool,
    debug_enabled: bool,
    translation_unit_growth: Cell<GrowthBudget>,
}

struct FunctionInfo {
    name: String,
    body: Option<ir::Function>,
    has_body: bool,
    native_definition: bool,
    bridge_definition: bool,
    internal_strong: bool,
    always_inline: bool,
    no_inline: bool,
    inline_hint: bool,
    exceptional: bool,
    recursive: bool,
    leaf: bool,
    user_named_global: bool,
    instruction_count: usize,
    block_count: usize,
}

impl InliningPlan {
    pub(super) fn new(
        module: &gir::FullModule,
        abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
        declarations: &Declarations,
        prepared: &[PreparedFunction],
        optimization: OptimizationLevel,
        debug_enabled: bool,
    ) -> Result<Self, CodegenError> {
        let recursive = recursive_functions(module);
        let heuristic_enabled =
            matches!(optimization, OptimizationLevel::O2 | OptimizationLevel::O3) && !debug_enabled;
        let prepared = prepared
            .iter()
            .map(|prepared| {
                let source = module.functions[prepared.function_index].id.0;
                (source, (prepared.id.as_u32(), &prepared.context.func))
            })
            .collect::<BTreeMap<_, _>>();

        let mut functions = BTreeMap::new();
        let mut object_to_source = BTreeMap::new();
        for function in &module.functions {
            let object_id = declarations
                .functions
                .get(&function.id.0)
                .copied()
                .map(cranelift_module::FuncId::as_u32);
            if let Some(object_id) = object_id {
                object_to_source.insert(object_id, function.id.0);
            } else if function.entry.is_some() {
                // Source-symbol reachability intentionally leaves an
                // unreferenced internal C `inline` definition without an
                // object declaration or prepared body.
                continue;
            }

            let boundary = abi_plan
                .plan()
                .definitions
                .get(&function.id)
                .map(|definition| &definition.boundary);
            let native_definition = matches!(boundary, Some(ccc_abi::BoundaryPlan::Native(_)));
            let bridge_definition = matches!(boundary, Some(ccc_abi::BoundaryPlan::Bridge(_)));
            let prepared_body = prepared
                .get(&function.id.0)
                .filter(|(definition_id, _)| Some(*definition_id) == object_id)
                .map(|(_, body)| *body);
            let has_body = prepared_body.is_some();
            let (leaf, instruction_count, block_count) =
                prepared_body.map(body_metrics).unwrap_or((false, 0, 0));
            let user_named_global = prepared_body.is_some_and(contains_user_named_global_value);
            let internal_strong =
                function.linkage == Linkage::Internal && function.binding == SymbolBinding::Strong;
            let always_inline = function.properties.always_inline;
            let no_inline = function.properties.no_inline;
            let exceptional = exceptional_frame_contract(function);
            let recursive = recursive.contains(&function.id.0);
            let body = prepared_body
                .filter(|_| {
                    native_definition
                        && !bridge_definition
                        && internal_strong
                        && !no_inline
                        && !exceptional
                        && !recursive
                        && leaf
                        && !user_named_global
                        && (always_inline || heuristic_enabled)
                })
                .cloned();

            functions.insert(
                function.id.0,
                FunctionInfo {
                    name: function.symbol_name.clone(),
                    body,
                    has_body,
                    native_definition,
                    bridge_definition,
                    internal_strong,
                    always_inline,
                    no_inline,
                    inline_hint: function.properties.inline,
                    exceptional,
                    recursive,
                    leaf,
                    user_named_global,
                    instruction_count,
                    block_count,
                },
            );
        }

        let plan = Self {
            functions,
            object_to_source,
            heuristic_enabled,
            debug_enabled,
            translation_unit_growth: Cell::new(GrowthBudget::default()),
        };
        plan.validate_required_calls(module)?;
        Ok(plan)
    }

    pub(super) fn policy(&self, caller: u32) -> InliningPolicy<'_> {
        InliningPolicy {
            plan: self,
            caller,
            caller_growth: GrowthBudget::default(),
        }
    }

    fn validate_required_calls(&self, module: &gir::FullModule) -> Result<(), CodegenError> {
        for caller in &module.functions {
            for block in &caller.blocks {
                for instruction in &block.instructions {
                    let gir::FullInstructionKind::DirectCall { function, .. } = &instruction.kind
                    else {
                        continue;
                    };
                    let Some(target) = self.functions.get(&function.0) else {
                        continue;
                    };
                    if !target.always_inline {
                        continue;
                    }
                    if let Some(reason) = self.structural_rejection(caller.id.0, target) {
                        return Err(inline_error(
                            format!(
                                "cannot honor `always_inline` call to `{}`: {reason}",
                                target.name
                            ),
                            instruction.span,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn structural_rejection(&self, caller: u32, target: &FunctionInfo) -> Option<&'static str> {
        let caller = self.functions.get(&caller)?;
        if self.debug_enabled {
            return Some("source-level inline debug information is not implemented");
        }
        if caller.bridge_definition {
            return Some("the caller is a generated ABI bridge body");
        }
        if caller.exceptional {
            return Some("the caller has a returns-twice frame contract");
        }
        if caller.recursive {
            return Some("the caller participates in recursion");
        }
        if target.no_inline {
            return Some("the definition is marked `noinline`");
        }
        if target.bridge_definition {
            return Some("the definition uses a generated ABI bridge");
        }
        if !target.native_definition || !target.has_body {
            return Some("no native translation-unit-local definition is available");
        }
        if !target.internal_strong {
            return Some("the definition is weak or has external linkage");
        }
        if target.exceptional {
            return Some("the definition has an exceptional frame contract");
        }
        if target.recursive {
            return Some("the definition participates in recursion");
        }
        if target.user_named_global {
            return Some("the definition references user-named global storage");
        }
        if !target.leaf {
            return Some("the first inlining policy accepts only leaf definitions");
        }
        None
    }
}

pub(super) struct InliningPolicy<'a> {
    plan: &'a InliningPlan,
    caller: u32,
    caller_growth: GrowthBudget,
}

impl InliningPolicy<'_> {
    pub(super) fn verify_required_calls_were_inlined(
        &self,
        caller: &ir::Function,
        source: &gir::FullFunction,
    ) -> Result<(), CodegenError> {
        for block in caller.layout.blocks() {
            for inst in caller.layout.block_insts(block) {
                let Some(callee) = direct_callee(caller, inst) else {
                    continue;
                };
                let Some(target) = resolve_target(self.plan, caller, callee) else {
                    continue;
                };
                if target.always_inline {
                    return Err(inline_error(
                        format!(
                            "cannot honor `always_inline` call to `{}`: Cranelift retained the direct call",
                            target.name
                        ),
                        source.span,
                    ));
                }
            }
        }
        Ok(())
    }

    fn exact_signature(
        &self,
        caller: &ir::Function,
        callee: ir::FuncRef,
        target: &FunctionInfo,
    ) -> bool {
        let Some(body) = target.body.as_ref() else {
            return false;
        };
        let external = &caller.dfg.ext_funcs[callee];
        body.signature == caller.dfg.signatures[external.signature]
    }

    fn heuristic_budget_allows(&self, target: &FunctionInfo) -> bool {
        callee_size_allows(
            target.instruction_count,
            target.block_count,
            target.inline_hint,
        ) && self
            .caller_growth
            .allows(target.instruction_count, target.block_count, CALLER_LIMITS)
            && self.plan.translation_unit_growth.get().allows(
                target.instruction_count,
                target.block_count,
                TRANSLATION_UNIT_LIMITS,
            )
    }

    fn record_inline(&mut self, target: &FunctionInfo) {
        self.caller_growth
            .record(target.instruction_count, target.block_count);
        let mut translation_unit = self.plan.translation_unit_growth.get();
        translation_unit.record(target.instruction_count, target.block_count);
        self.plan.translation_unit_growth.set(translation_unit);
    }
}

fn callee_size_allows(instructions: usize, blocks: usize, inline_hint: bool) -> bool {
    let instruction_limit = if inline_hint {
        MAX_INLINE_HINT_CALLEE_INSTRUCTIONS
    } else {
        MAX_HEURISTIC_CALLEE_INSTRUCTIONS
    };
    instructions <= instruction_limit && blocks <= MAX_HEURISTIC_CALLEE_BLOCKS
}

#[derive(Clone, Copy, Default)]
struct GrowthBudget {
    sites: usize,
    instruction_growth: usize,
    block_growth: usize,
}

#[derive(Clone, Copy)]
struct GrowthLimits {
    sites: usize,
    instructions: usize,
    blocks: usize,
}

impl GrowthBudget {
    fn allows(self, instructions: usize, blocks: usize, limits: GrowthLimits) -> bool {
        // Cranelift replaces the original call with a jump, clones every
        // callee instruction and block, and normally splits out a continuation
        // block. Charge that full structural growth rather than assuming that
        // the call or callee return disappears during this transformation.
        let block_growth = blocks.saturating_add(1);
        self.sites < limits.sites
            && self.instruction_growth.saturating_add(instructions) <= limits.instructions
            && self.block_growth.saturating_add(block_growth) <= limits.blocks
    }

    fn record(&mut self, instructions: usize, blocks: usize) {
        self.sites = self.sites.saturating_add(1);
        self.instruction_growth = self.instruction_growth.saturating_add(instructions);
        self.block_growth = self.block_growth.saturating_add(blocks.saturating_add(1));
    }
}

impl Inline for InliningPolicy<'_> {
    fn inline(
        &mut self,
        caller: &ir::Function,
        _call_inst: ir::Inst,
        call_opcode: Opcode,
        callee: ir::FuncRef,
        _call_args: &[ir::Value],
    ) -> InlineCommand<'_> {
        if !matches!(call_opcode, Opcode::Call | Opcode::ReturnCall) {
            return InlineCommand::KeepCall;
        }
        let Some(target) = resolve_target(self.plan, caller, callee) else {
            return InlineCommand::KeepCall;
        };
        if self
            .plan
            .structural_rejection(self.caller, target)
            .is_some()
            || !self.exact_signature(caller, callee, target)
        {
            return InlineCommand::KeepCall;
        }

        let should_inline = target.always_inline
            || self.plan.heuristic_enabled && self.heuristic_budget_allows(target);
        if !should_inline {
            return InlineCommand::KeepCall;
        }
        self.record_inline(target);
        InlineCommand::Inline {
            callee: Cow::Borrowed(target.body.as_ref().expect("eligible callee has a body")),
            visit_callee: false,
        }
    }
}

fn resolve_target<'a>(
    plan: &'a InliningPlan,
    caller: &ir::Function,
    callee: ir::FuncRef,
) -> Option<&'a FunctionInfo> {
    let external = &caller.dfg.ext_funcs[callee];
    let ExternalName::User(name) = &external.name else {
        return None;
    };
    let name = &caller.params.user_named_funcs()[*name];
    if name.namespace != 0 {
        return None;
    }
    let source = plan.object_to_source.get(&name.index)?;
    plan.functions.get(source)
}

fn direct_callee(function: &ir::Function, inst: ir::Inst) -> Option<ir::FuncRef> {
    match function.dfg.insts[inst] {
        ir::InstructionData::Call { func_ref, .. }
        | ir::InstructionData::TryCall { func_ref, .. } => Some(func_ref),
        _ => None,
    }
}

fn body_metrics(function: &ir::Function) -> (bool, usize, usize) {
    let mut leaf = true;
    let mut instructions = 0;
    let blocks = function.layout.blocks().count();
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            instructions += 1;
            leaf &= !function.dfg.insts[inst].opcode().is_call();
        }
    }
    (leaf, instructions, blocks)
}

fn contains_user_named_global_value(function: &ir::Function) -> bool {
    function.global_values.values().any(|value| {
        matches!(
            value,
            ir::GlobalValueData::Symbol {
                name: ExternalName::User(_),
                ..
            }
        )
    })
}

fn exceptional_frame_contract(function: &gir::FullFunction) -> bool {
    function.properties.no_return
        || function.properties.returns_twice
        || function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    &instruction.kind,
                    gir::FullInstructionKind::DirectCall { effects, .. }
                        | gir::FullInstructionKind::IndirectCall { effects, .. }
                        if effects.returns_twice
                )
            })
        })
}

fn recursive_functions(module: &gir::FullModule) -> BTreeSet<u32> {
    let mut definitions = module
        .functions
        .iter()
        .filter(|function| function.entry.is_some())
        .map(|function| function.id.0)
        .collect::<Vec<_>>();
    definitions.sort_unstable();
    let indices = definitions
        .iter()
        .copied()
        .enumerate()
        .map(|(index, function)| (function, index))
        .collect::<HashMap<_, _>>();
    let mut graph = vec![Vec::new(); definitions.len()];
    for function in module
        .functions
        .iter()
        .filter(|function| function.entry.is_some())
    {
        let Some(&caller) = indices.get(&function.id.0) else {
            continue;
        };
        graph[caller] = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.kind {
                gir::FullInstructionKind::DirectCall { function, .. } => {
                    indices.get(&function.0).copied()
                }
                _ => None,
            })
            .collect();
        graph[caller].sort_unstable();
        graph[caller].dedup();
    }

    recursive_components(&graph)
        .into_iter()
        .enumerate()
        .filter(|(_, recursive)| *recursive)
        .map(|(index, _)| definitions[index])
        .collect()
}

/// Classify recursive strongly connected components with an iterative
/// Kosaraju traversal. This is linear in the call graph and cannot overflow
/// the compiler stack on a long generated call chain.
fn recursive_components(graph: &[Vec<usize>]) -> Vec<bool> {
    let mut reverse = vec![Vec::new(); graph.len()];
    for (caller, callees) in graph.iter().enumerate() {
        for &callee in callees {
            reverse[callee].push(caller);
        }
    }

    let mut visited = vec![false; graph.len()];
    let mut finish = Vec::with_capacity(graph.len());
    for start in 0..graph.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0)];
        while let Some((node, next_edge)) = stack.last_mut() {
            if *next_edge < graph[*node].len() {
                let callee = graph[*node][*next_edge];
                *next_edge += 1;
                if !visited[callee] {
                    visited[callee] = true;
                    stack.push((callee, 0));
                }
            } else {
                finish.push(*node);
                stack.pop();
            }
        }
    }

    let mut assigned = vec![false; graph.len()];
    let mut recursive = vec![false; graph.len()];
    for start in finish.into_iter().rev() {
        if assigned[start] {
            continue;
        }
        assigned[start] = true;
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node);
            for &caller in &reverse[node] {
                if !assigned[caller] {
                    assigned[caller] = true;
                    stack.push(caller);
                }
            }
        }
        if component.len() > 1 || graph[start].contains(&start) {
            for node in component {
                recursive[node] = true;
            }
        }
    }
    recursive
}

#[cfg(test)]
mod call_graph_tests {
    use super::recursive_components;

    #[test]
    fn iterative_scc_classification_handles_cycles_and_long_chains() {
        let graph = vec![vec![1], vec![2], vec![0, 3], vec![4], vec![]];
        assert_eq!(
            recursive_components(&graph),
            [true, true, true, false, false]
        );

        let mut chain = (0..10_000).map(|node| vec![node + 1]).collect::<Vec<_>>();
        chain.push(Vec::new());
        assert!(
            recursive_components(&chain)
                .into_iter()
                .all(|recursive| !recursive)
        );
    }
}

fn inline_error(message: impl Into<String>, span: ccc_session::Span) -> CodegenError {
    CodegenError {
        code: INLINE_ERROR,
        message: message.into(),
        span: Some(span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_heuristic_budget_boundaries_are_inclusive() {
        assert!(callee_size_allows(
            MAX_HEURISTIC_CALLEE_INSTRUCTIONS,
            1,
            false
        ));
        assert!(!callee_size_allows(
            MAX_HEURISTIC_CALLEE_INSTRUCTIONS + 1,
            1,
            false
        ));
        assert!(callee_size_allows(
            MAX_INLINE_HINT_CALLEE_INSTRUCTIONS,
            1,
            true
        ));
        assert!(!callee_size_allows(
            MAX_INLINE_HINT_CALLEE_INSTRUCTIONS + 1,
            1,
            true
        ));
        assert!(callee_size_allows(1, MAX_HEURISTIC_CALLEE_BLOCKS, false));
        assert!(!callee_size_allows(
            1,
            MAX_HEURISTIC_CALLEE_BLOCKS + 1,
            false
        ));

        let last_site = GrowthBudget {
            sites: MAX_HEURISTIC_SITES_PER_CALLER - 1,
            ..GrowthBudget::default()
        };
        assert!(last_site.allows(1, 1, CALLER_LIMITS));
        let no_sites = GrowthBudget {
            sites: MAX_HEURISTIC_SITES_PER_CALLER,
            ..GrowthBudget::default()
        };
        assert!(!no_sites.allows(1, 1, CALLER_LIMITS));

        let instruction_edge = GrowthBudget {
            instruction_growth: MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_CALLER
                - MAX_HEURISTIC_CALLEE_INSTRUCTIONS,
            ..GrowthBudget::default()
        };
        assert!(instruction_edge.allows(MAX_HEURISTIC_CALLEE_INSTRUCTIONS, 1, CALLER_LIMITS));
        let instruction_over = GrowthBudget {
            instruction_growth: instruction_edge.instruction_growth + 1,
            ..GrowthBudget::default()
        };
        assert!(!instruction_over.allows(MAX_HEURISTIC_CALLEE_INSTRUCTIONS, 1, CALLER_LIMITS));

        let block_edge = GrowthBudget {
            block_growth: MAX_HEURISTIC_BLOCK_GROWTH_PER_CALLER - (MAX_HEURISTIC_CALLEE_BLOCKS + 1),
            ..GrowthBudget::default()
        };
        assert!(block_edge.allows(1, MAX_HEURISTIC_CALLEE_BLOCKS, CALLER_LIMITS));
        let block_over = GrowthBudget {
            block_growth: block_edge.block_growth + 1,
            ..GrowthBudget::default()
        };
        assert!(!block_over.allows(1, MAX_HEURISTIC_CALLEE_BLOCKS, CALLER_LIMITS));

        let translation_unit_edge = GrowthBudget {
            sites: MAX_HEURISTIC_SITES_PER_TRANSLATION_UNIT - 1,
            instruction_growth: MAX_HEURISTIC_INSTRUCTION_GROWTH_PER_TRANSLATION_UNIT - 1,
            block_growth: MAX_HEURISTIC_BLOCK_GROWTH_PER_TRANSLATION_UNIT - 2,
        };
        assert!(translation_unit_edge.allows(1, 1, TRANSLATION_UNIT_LIMITS));
        let translation_unit_full = GrowthBudget {
            sites: MAX_HEURISTIC_SITES_PER_TRANSLATION_UNIT,
            ..GrowthBudget::default()
        };
        assert!(!translation_unit_full.allows(1, 1, TRANSLATION_UNIT_LIMITS));
    }
}
