use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ccc_target::OptimizationLevel;

use super::lower::compact_values;
use super::verify::{instruction_operands, terminator_operands};
use super::{
    BlockId, FullEdge, FullFunction, FullInstructionKind, FullModule, FullTerminator, IrError,
    ScalarConstant, ScalarConversion, UnaryOperation, ValueId, verify_frontend,
};

/// Runs the C-aware cleanup pipeline and validates the IR on both boundaries.
///
/// Generic instruction selection and machine-level optimization remain the
/// responsibility of the backend. This pipeline only simplifies CCC's own
/// control-flow and effect representation.
pub fn optimize_frontend(
    module: &mut FullModule,
    optimization: OptimizationLevel,
) -> Result<(), IrError> {
    verify_frontend(module)?;
    if optimization.optimizes() {
        for function in &mut module.functions {
            optimize_function(function)?;
        }
    }
    verify_frontend(module)
}

fn optimize_function(function: &mut FullFunction) -> Result<(), IrError> {
    if function.entry.is_none() {
        return Ok(());
    }

    loop {
        let mut changed = sparsify_block_parameters(function)?;
        changed |= simplify_control_flow(function)?;
        changed |= eliminate_dead_pure_instructions(function)?;
        if !changed {
            break;
        }
    }
    Ok(())
}

fn sparsify_block_parameters(function: &mut FullFunction) -> Result<bool, IrError> {
    let mut incoming = vec![Vec::<Vec<ValueId>>::new(); function.blocks.len()];
    for block in &function.blocks {
        if let Some(terminator) = &block.terminator {
            for edge in terminator_edges(terminator) {
                incoming[edge.target.0 as usize].push(edge.arguments.clone());
            }
        }
    }

    let mut aliases = BTreeMap::<ValueId, ValueId>::new();
    let entry = function.entry;
    loop {
        let mut discovered = false;
        for block in &function.blocks {
            if Some(block.id) == entry {
                continue;
            }
            for (index, parameter) in block.parameters.iter().copied().enumerate() {
                if aliases.contains_key(&parameter) {
                    continue;
                }
                let mut replacement = None;
                let mut is_trivial = true;
                for arguments in &incoming[block.id.0 as usize] {
                    let argument = resolve_alias(arguments[index], &aliases)?;
                    if argument == parameter {
                        continue;
                    }
                    match replacement {
                        None => replacement = Some(argument),
                        Some(candidate) if candidate == argument => {}
                        Some(_) => {
                            is_trivial = false;
                            break;
                        }
                    }
                }
                if is_trivial && let Some(replacement) = replacement {
                    aliases.insert(parameter, replacement);
                    discovered = true;
                }
            }
        }
        if !discovered {
            break;
        }
    }

    if aliases.is_empty() {
        return Ok(false);
    }

    let removed = function
        .blocks
        .iter()
        .map(|block| {
            block
                .parameters
                .iter()
                .map(|parameter| aliases.contains_key(parameter))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    for block in &mut function.blocks {
        let mask = &removed[block.id.0 as usize];
        block.parameters = std::mem::take(&mut block.parameters)
            .into_iter()
            .enumerate()
            .filter_map(|(index, parameter)| (!mask[index]).then_some(parameter))
            .collect();
    }
    for block in &mut function.blocks {
        if let Some(terminator) = &mut block.terminator {
            for edge in terminator_edges_mut(terminator) {
                let mask = &removed[edge.target.0 as usize];
                edge.arguments = std::mem::take(&mut edge.arguments)
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, argument)| (!mask[index]).then_some(argument))
                    .collect();
            }
        }
    }
    compact_values(function, &aliases)?;
    Ok(true)
}

fn simplify_control_flow(function: &mut FullFunction) -> Result<bool, IrError> {
    let constants = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(
            |instruction| match (instruction.result, &instruction.kind) {
                (Some(result), FullInstructionKind::Constant(constant)) => {
                    Some((result, *constant))
                }
                _ => None,
            },
        )
        .collect::<BTreeMap<_, _>>();
    let mut truths = constants
        .iter()
        .filter_map(|(value, constant)| constant_truth(constant).map(|truth| (*value, truth)))
        .collect::<BTreeMap<_, _>>();
    loop {
        let mut discovered = false;
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            let Some(result) = instruction.result else {
                continue;
            };
            let truth = match &instruction.kind {
                FullInstructionKind::Convert {
                    kind: ScalarConversion::ToBoolean,
                    operand,
                    ..
                } => truths.get(operand).copied(),
                FullInstructionKind::Unary {
                    operator: UnaryOperation::LogicalNot,
                    operand,
                } => truths.get(operand).map(|truth| !truth),
                _ => None,
            };
            if let Some(truth) = truth
                && truths.insert(result, truth).is_none()
            {
                discovered = true;
            }
        }
        if !discovered {
            break;
        }
    }

    let mut changed = false;
    for block in &mut function.blocks {
        let replacement = match block.terminator.as_ref() {
            Some(FullTerminator::Conditional {
                condition: _,
                then_edge,
                else_edge,
            }) if then_edge == else_edge => Some(FullTerminator::Branch(then_edge.clone())),
            Some(FullTerminator::Conditional {
                condition,
                then_edge,
                else_edge,
            }) => truths.get(condition).copied().map(|truth| {
                FullTerminator::Branch(if truth {
                    then_edge.clone()
                } else {
                    else_edge.clone()
                })
            }),
            Some(FullTerminator::Switch {
                selector,
                cases,
                default,
            }) => constants
                .get(selector)
                .and_then(constant_integer_bits)
                .map(|value| {
                    FullTerminator::Branch(
                        cases
                            .iter()
                            .find(|case| case.value == value)
                            .map_or_else(|| default.clone(), |case| case.edge.clone()),
                    )
                }),
            _ => None,
        };
        if let Some(replacement) = replacement {
            block.terminator = Some(replacement);
            changed = true;
        }
    }

    let removed_blocks = remove_unreachable_blocks(function)?;
    if removed_blocks {
        compact_values(function, &BTreeMap::new())?;
    }
    Ok(changed || removed_blocks)
}

fn remove_unreachable_blocks(function: &mut FullFunction) -> Result<bool, IrError> {
    let Some(entry) = function.entry else {
        return Ok(false);
    };
    let mut reachable = vec![false; function.blocks.len()];
    let mut pending = VecDeque::from([entry]);
    while let Some(block) = pending.pop_front() {
        let index = block.0 as usize;
        if reachable[index] {
            continue;
        }
        reachable[index] = true;
        if let Some(terminator) = &function.blocks[index].terminator {
            pending.extend(
                terminator_edges(terminator)
                    .into_iter()
                    .map(|edge| edge.target),
            );
        }
    }
    if reachable.iter().all(|reachable| *reachable) {
        return Ok(false);
    }

    let mut remap = vec![None; function.blocks.len()];
    let mut next = 0u32;
    for (index, retained) in reachable.iter().copied().enumerate() {
        if retained {
            remap[index] = Some(BlockId(next));
            next = next
                .checked_add(1)
                .ok_or_else(|| IrError::verify("block id space exhausted during optimization"))?;
        }
    }

    let old_blocks = std::mem::take(&mut function.blocks);
    for (index, mut block) in old_blocks.into_iter().enumerate() {
        let Some(new_id) = remap[index] else {
            continue;
        };
        block.id = new_id;
        if let Some(terminator) = &mut block.terminator {
            for edge in terminator_edges_mut(terminator) {
                edge.target = remap[edge.target.0 as usize].ok_or_else(|| {
                    IrError::verify("reachable block targets a removed block during optimization")
                })?;
            }
        }
        function.blocks.push(block);
    }
    function.entry = remap[entry.0 as usize];
    Ok(true)
}

fn eliminate_dead_pure_instructions(function: &mut FullFunction) -> Result<bool, IrError> {
    let mut changed = false;
    loop {
        let mut uses = vec![0usize; function.value_types.len()];
        for parameter in &function.parameters {
            if let Some(incoming) = parameter.incoming {
                uses[incoming.0 as usize] += 1;
            }
        }
        for block in &function.blocks {
            for instruction in &block.instructions {
                for operand in instruction_operands(&instruction.kind) {
                    uses[operand.0 as usize] += 1;
                }
            }
            if let Some(terminator) = &block.terminator {
                for operand in terminator_operands(terminator) {
                    uses[operand.0 as usize] += 1;
                }
            }
        }

        let mut removed = false;
        for block in &mut function.blocks {
            block.instructions.retain(|instruction| {
                let discard = instruction.result.is_some_and(|result| {
                    uses[result.0 as usize] == 0 && is_pure(&instruction.kind)
                });
                removed |= discard;
                !discard
            });
        }
        if !removed {
            break;
        }
        changed = true;
    }

    if changed {
        compact_values(function, &BTreeMap::new())?;
    }
    Ok(changed)
}

fn is_pure(kind: &FullInstructionKind) -> bool {
    matches!(
        kind,
        FullInstructionKind::Constant(_)
            | FullInstructionKind::AddressConstant { .. }
            | FullInstructionKind::AddressOfGlobal { .. }
            | FullInstructionKind::AddressOfFunction { .. }
            | FullInstructionKind::AddressOfString { .. }
            | FullInstructionKind::AddressOfStorage { .. }
            | FullInstructionKind::ProjectField { .. }
            | FullInstructionKind::PointerOffset { .. }
            | FullInstructionKind::PointerDifference { .. }
            | FullInstructionKind::AggregateProject { .. }
            | FullInstructionKind::Convert { .. }
            | FullInstructionKind::Unary { .. }
            | FullInstructionKind::Binary { .. }
            | FullInstructionKind::IntegerIntrinsic { .. }
    ) || matches!(
        kind,
        FullInstructionKind::Load { access, .. }
            | FullInstructionKind::BitfieldLoad { access, .. }
            | FullInstructionKind::AggregateSnapshot { access, .. }
            if !access_is_ordered(*access)
    )
}

fn access_is_ordered(access: super::MemoryAccess) -> bool {
    access.volatile || access.atomic.is_some() || access.non_elidable || access.non_movable
}

fn constant_truth(constant: &ScalarConstant) -> Option<bool> {
    match constant {
        ScalarConstant::Signed(value) => Some(*value != 0),
        ScalarConstant::Unsigned(value) => Some(*value != 0),
        ScalarConstant::Floating(value) => Some(*value != 0.0),
        ScalarConstant::NullPointer => Some(false),
        ScalarConstant::LongDouble(_) => None,
    }
}

fn constant_integer_bits(constant: &ScalarConstant) -> Option<u128> {
    match constant {
        ScalarConstant::Signed(value) => Some(*value as u128),
        ScalarConstant::Unsigned(value) => Some(*value),
        ScalarConstant::Floating(_)
        | ScalarConstant::LongDouble(_)
        | ScalarConstant::NullPointer => None,
    }
}

fn resolve_alias(
    mut value: ValueId,
    aliases: &BTreeMap<ValueId, ValueId>,
) -> Result<ValueId, IrError> {
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases.get(&value).copied() {
        if !seen.insert(value) {
            return Err(IrError::verify(
                "cyclic block-parameter alias during optimization",
            ));
        }
        value = next;
    }
    Ok(value)
}

fn terminator_edges(terminator: &FullTerminator) -> Vec<&FullEdge> {
    match terminator {
        FullTerminator::Branch(edge) => vec![edge],
        FullTerminator::Conditional {
            then_edge,
            else_edge,
            ..
        } => vec![then_edge, else_edge],
        FullTerminator::Switch { cases, default, .. } => cases
            .iter()
            .map(|case| &case.edge)
            .chain(std::iter::once(default))
            .collect(),
        FullTerminator::IndirectBranch { targets, .. } => targets.iter().collect(),
        FullTerminator::Return(_) | FullTerminator::Unreachable => Vec::new(),
    }
}

fn terminator_edges_mut(terminator: &mut FullTerminator) -> Vec<&mut FullEdge> {
    match terminator {
        FullTerminator::Branch(edge) => vec![edge],
        FullTerminator::Conditional {
            then_edge,
            else_edge,
            ..
        } => vec![then_edge, else_edge],
        FullTerminator::Switch { cases, default, .. } => cases
            .iter_mut()
            .map(|case| &mut case.edge)
            .chain(std::iter::once(default))
            .collect(),
        FullTerminator::IndirectBranch { targets, .. } => targets.iter_mut().collect(),
        FullTerminator::Return(_) | FullTerminator::Unreachable => Vec::new(),
    }
}
