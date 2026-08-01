use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ccc_target::{EffectiveCompilationConfig, OptimizationLevel};
use ccc_types::{QualifiedType, RecordKind, TypeKind, TypeStore};

mod cse;
mod effects;
mod fold;

#[cfg(test)]
mod tests;

use super::lower::{compact_values, remap_promoted_local_update_positions};
use super::verify::{instruction_operands, terminator_operands};
use super::{
    BlockId, FullEdge, FullFunction, FullInstructionKind, FullModule, FullTerminator, IrError,
    MemoryAccess, MemoryResidencyReason, ScalarConstant, ScalarConversion, StorageId,
    StorageLocation, UnaryOperation, ValueId, verify_frontend,
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
    let config = EffectiveCompilationConfig::default().with_optimization_level(optimization);
    optimize_frontend_for_config(module, &config)
}

/// Runs target-aware C IR cleanup using the complete effective configuration.
///
/// Callers that already resolved a target should use this entry point so
/// integer folding observes that target's exact widths and signedness rules.
pub fn optimize_frontend_for_config(
    module: &mut FullModule,
    config: &EffectiveCompilationConfig,
) -> Result<(), IrError> {
    verify_frontend(module)?;
    let profile = OptimizationProfile::for_level(config.optimization);
    if profile.enabled {
        let types = &module.types;
        for function in &mut module.functions {
            optimize_function(types, function, config, profile)?;
        }
    }
    verify_frontend(module)
}

#[derive(Clone, Copy)]
struct OptimizationProfile {
    enabled: bool,
    local_cse: bool,
}

impl OptimizationProfile {
    const fn for_level(level: OptimizationLevel) -> Self {
        match level {
            OptimizationLevel::O0 => Self {
                enabled: false,
                local_cse: false,
            },
            OptimizationLevel::O1 => Self {
                enabled: true,
                local_cse: false,
            },
            OptimizationLevel::O2
            | OptimizationLevel::O3
            | OptimizationLevel::Size
            | OptimizationLevel::SizeMin => Self {
                enabled: true,
                local_cse: true,
            },
        }
    }
}

fn optimize_function(
    types: &TypeStore,
    function: &mut FullFunction,
    config: &EffectiveCompilationConfig,
    profile: OptimizationProfile,
) -> Result<(), IrError> {
    if function.entry.is_none() {
        return Ok(());
    }

    // Every pass in this pipeline either removes an entity or replaces an
    // instruction without growing the function. Use the input size to place a
    // hard ceiling on accidental pass interaction while still allowing chains
    // of newly exposed simplifications to settle.
    let input_size = function
        .blocks
        .iter()
        .map(|block| block.parameters.len() + block.instructions.len() + 1)
        .sum::<usize>();
    let iteration_limit = input_size.saturating_mul(2).max(8);
    for _ in 0..iteration_limit {
        let mut changed = fold::fold_constants(types, function, config);
        changed |= sparsify_block_parameters(function)
            .map_err(|error| pass_failure(error, "block-parameter cleanup"))?;
        changed |= simplify_control_flow(types, function, config)
            .map_err(|error| pass_failure(error, "control-flow cleanup"))?;
        changed |= forward_empty_blocks(function)
            .map_err(|error| pass_failure(error, "empty-block forwarding"))?;
        if profile.local_cse {
            changed |= forward_automatic_aggregate_field_loads(types, function)
                .map_err(|error| pass_failure(error, "automatic aggregate field forwarding"))?;
            changed |= elide_immediate_aggregate_snapshots(function)
                .map_err(|error| pass_failure(error, "aggregate snapshot cleanup"))?;
            changed |= cse::eliminate_common_expressions(function)
                .map_err(|error| pass_failure(error, "local common-expression cleanup"))?;
        }
        changed |= eliminate_dead_pure_instructions(function)
            .map_err(|error| pass_failure(error, "dead-instruction cleanup"))?;
        if !changed {
            return Ok(());
        }
    }
    Err(IrError::verify(format!(
        "C IR optimization did not converge within {iteration_limit} iterations"
    )))
}

fn pass_failure(mut error: IrError, pass: &str) -> IrError {
    error.message = format!("{pass}: {}", error.message);
    error
}

/// Collapses an aggregate lvalue snapshot consumed immediately by one copy.
///
/// The aggregate copy backend reads all of its source before writing any
/// destination byte, so it preserves the snapshot's value semantics even when
/// the ranges alias. Keeping this rewrite adjacent also preserves the
/// source-read ordering with respect to every other effect.
fn elide_immediate_aggregate_snapshots(function: &mut FullFunction) -> Result<bool, IrError> {
    let use_counts = super::value_use_counts(function);
    let mut changed = false;
    let mut retained_instructions = Vec::with_capacity(function.blocks.len());
    for block in &mut function.blocks {
        let mut pending = std::mem::take(&mut block.instructions)
            .into_iter()
            .collect::<VecDeque<_>>();
        let mut retained = Vec::with_capacity(pending.len());
        let mut retained_mask = Vec::with_capacity(pending.len());
        while let Some(snapshot) = pending.pop_front() {
            let shortcut = match (&snapshot.result, &snapshot.kind) {
                (
                    Some(snapshot_result),
                    FullInstructionKind::AggregateSnapshot { source, access, .. },
                ) => {
                    if let Some(copy) = pending.front_mut()
                        && let FullInstructionKind::AggregateCopy {
                            source: copy_source,
                            source_access,
                            ..
                        } = &mut copy.kind
                        && *copy_source == *snapshot_result
                        && *source_access == MemoryAccess::default()
                        && use_counts[snapshot_result.0 as usize] == 1
                    {
                        *copy_source = *source;
                        *source_access = *access;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if shortcut {
                retained_mask.push(false);
                retained.push(
                    pending
                        .pop_front()
                        .expect("aggregate snapshot cleanup lost its adjacent copy"),
                );
                retained_mask.push(true);
                changed = true;
            } else {
                retained.push(snapshot);
                retained_mask.push(true);
            }
        }
        block.instructions = retained;
        retained_instructions.push(retained_mask);
    }
    if changed {
        remap_promoted_local_update_positions(function, &retained_instructions)?;
        compact_values(function, &BTreeMap::new())?;
    }
    Ok(changed)
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct AutomaticAggregateField {
    storage: StorageId,
    fields: Vec<usize>,
}

#[derive(Clone)]
struct AutomaticAggregateAddress {
    field: AutomaticAggregateField,
    forwardable: bool,
}

/// Finds direct field addresses within automatic aggregates after proving that
/// every derived address remains in a modeled, non-escaping use. Residency
/// metadata cheaply excludes normal address-taken objects, while the use proof
/// covers malformed or underclassified IR as well. Only struct paths are
/// forwardable; union paths remain tracked solely to detect escapes.
fn automatic_aggregate_field_locations(
    types: &TypeStore,
    function: &FullFunction,
) -> BTreeMap<ValueId, AutomaticAggregateAddress> {
    let storages = function
        .storage
        .iter()
        .filter(|storage| {
            storage.location == StorageLocation::Automatic
                && storage.required_by.as_slice() == [MemoryResidencyReason::Aggregate]
        })
        .map(|storage| storage.id)
        .collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();

    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        let Some(value) = instruction.result else {
            continue;
        };
        match &instruction.kind {
            FullInstructionKind::AddressOfStorage { storage } if storages.contains(storage) => {
                result.insert(
                    value,
                    AutomaticAggregateAddress {
                        field: AutomaticAggregateField {
                            storage: *storage,
                            fields: Vec::new(),
                        },
                        forwardable: true,
                    },
                );
            }
            FullInstructionKind::ProjectField {
                base,
                record,
                field_index,
                ..
            } => {
                let Some(mut location) = result.get(base).cloned() else {
                    continue;
                };
                let Some(TypeKind::Record(record_id)) = types.try_kind(record.ty) else {
                    continue;
                };
                let forwardable = location.forwardable
                    && types
                        .record(*record_id)
                        .is_some_and(|definition| definition.kind == RecordKind::Struct);
                location.field.fields.push(*field_index);
                result.insert(
                    value,
                    AutomaticAggregateAddress {
                        field: location.field,
                        forwardable,
                    },
                );
            }
            _ => {}
        }
    }

    let mut escaped_storages = BTreeSet::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if !has_only_modeled_aggregate_address_uses(&instruction.kind, &result) {
            for value in instruction_operands(&instruction.kind) {
                if let Some(location) = result.get(&value) {
                    escaped_storages.insert(location.field.storage);
                }
            }
        }
    }
    for terminator in function
        .blocks
        .iter()
        .filter_map(|block| block.terminator.as_ref())
    {
        for value in terminator_operands(terminator) {
            if let Some(location) = result.get(&value) {
                escaped_storages.insert(location.field.storage);
            }
        }
    }
    result.retain(|_, location| !escaped_storages.contains(&location.field.storage));
    result
}

/// Returns true only for exact direct-address consumers whose effects are
/// modeled below. Everything else is an escape, including conversions and
/// pointer arithmetic, so malformed metadata cannot grant a no-alias proof.
fn has_only_modeled_aggregate_address_uses(
    kind: &FullInstructionKind,
    locations: &BTreeMap<ValueId, AutomaticAggregateAddress>,
) -> bool {
    let is_location = |value| locations.contains_key(&value);
    match kind {
        FullInstructionKind::ProjectField { .. }
        | FullInstructionKind::Load { .. }
        | FullInstructionKind::BitfieldLoad { .. }
        | FullInstructionKind::ZeroInitialize { .. }
        | FullInstructionKind::StringInitialize { .. }
        | FullInstructionKind::AggregateCopy { .. }
        | FullInstructionKind::AggregateSnapshot { .. }
        | FullInstructionKind::MemoryCopy { .. }
        | FullInstructionKind::MemorySet { .. }
        | FullInstructionKind::X86Rdtsc { .. }
        | FullInstructionKind::VaStart { .. }
        | FullInstructionKind::VaArg { .. }
        | FullInstructionKind::VaEnd { .. } => true,
        FullInstructionKind::Store { value, .. }
        | FullInstructionKind::BitfieldStore { value, .. } => !is_location(*value),
        FullInstructionKind::AtomicReadModifyWrite { operand, .. } => !is_location(*operand),
        FullInstructionKind::AtomicCompareExchange {
            expected,
            replacement,
            ..
        } => !is_location(*expected) && !is_location(*replacement),
        FullInstructionKind::X86Cpuid { leaf, subleaf, .. } => {
            !is_location(*leaf) && !subleaf.is_some_and(is_location)
        }
        FullInstructionKind::VaCopy { source, .. } => !is_location(*source),
        _ => !instruction_operands(kind).into_iter().any(is_location),
    }
}

fn is_forwardable_scalar(types: &TypeStore, object: QualifiedType) -> bool {
    object.qualifiers.is_empty()
        && (types.is_arithmetic(object.ty)
            || matches!(types.try_kind(object.ty), Some(TypeKind::Pointer(_))))
}

fn invalidate_automatic_aggregate_field(
    values: &mut BTreeMap<AutomaticAggregateField, (ValueId, QualifiedType)>,
    locations: &BTreeMap<ValueId, AutomaticAggregateAddress>,
    address: ValueId,
) {
    let Some(destination) = locations.get(&address) else {
        return;
    };
    values.retain(|location, _| {
        location.storage != destination.field.storage
            || !location.fields.starts_with(&destination.field.fields)
    });
}

fn invalidate_automatic_aggregate_storage(
    values: &mut BTreeMap<AutomaticAggregateField, (ValueId, QualifiedType)>,
    locations: &BTreeMap<ValueId, AutomaticAggregateAddress>,
    address: ValueId,
) {
    let Some(destination) = locations.get(&address) else {
        return;
    };
    values.retain(|location, _| location.storage != destination.field.storage);
}

/// Forwards scalar loads from a preceding store to the same direct field of a
/// non-escaping automatic aggregate in one basic block.
///
/// Calls and unknown pointers cannot modify these aggregates because their
/// addresses never escape. Writes with a recognized aggregate destination
/// invalidate its mapped field range; unions, volatile/atomic accesses, and
/// address-taken storage are excluded entirely.
fn forward_automatic_aggregate_field_loads(
    types: &TypeStore,
    function: &mut FullFunction,
) -> Result<bool, IrError> {
    let locations = automatic_aggregate_field_locations(types, function);
    if locations.is_empty() {
        return Ok(false);
    }

    let mut aliases = BTreeMap::new();
    let mut retained_instructions = Vec::with_capacity(function.blocks.len());
    for block in &function.blocks {
        let mut values = BTreeMap::<AutomaticAggregateField, (ValueId, QualifiedType)>::new();
        let mut retained = Vec::with_capacity(block.instructions.len());
        for instruction in &block.instructions {
            let mut forwarded = false;
            match &instruction.kind {
                FullInstructionKind::Load {
                    address,
                    object,
                    access,
                } => {
                    if *access == MemoryAccess::default()
                        && is_forwardable_scalar(types, *object)
                        && let Some(location) = locations.get(address)
                        && location.forwardable
                        && !location.field.fields.is_empty()
                        && let Some((value, stored_object)) = values.get(&location.field)
                        && *stored_object == *object
                        && let Some(result) = instruction.result
                    {
                        aliases.insert(result, *value);
                        forwarded = true;
                    }
                }
                FullInstructionKind::Store {
                    address,
                    value,
                    object,
                    access,
                } => {
                    invalidate_automatic_aggregate_field(&mut values, &locations, *address);
                    if *access == MemoryAccess::default()
                        && is_forwardable_scalar(types, *object)
                        && let Some(location) = locations.get(address)
                        && location.forwardable
                        && !location.field.fields.is_empty()
                    {
                        values.insert(
                            location.field.clone(),
                            (resolve_alias(*value, &aliases)?, *object),
                        );
                    }
                }
                FullInstructionKind::MemoryCopy { destination, .. }
                | FullInstructionKind::MemorySet { destination, .. } => {
                    invalidate_automatic_aggregate_storage(&mut values, &locations, *destination);
                }
                FullInstructionKind::ZeroInitialize { destination, .. }
                | FullInstructionKind::StringInitialize { destination, .. }
                | FullInstructionKind::AggregateCopy { destination, .. }
                | FullInstructionKind::BitfieldStore {
                    address: destination,
                    ..
                }
                | FullInstructionKind::AtomicReadModifyWrite {
                    address: destination,
                    ..
                }
                | FullInstructionKind::AtomicCompareExchange {
                    address: destination,
                    ..
                }
                | FullInstructionKind::VaCopy { destination, .. } => {
                    invalidate_automatic_aggregate_field(&mut values, &locations, *destination);
                }
                FullInstructionKind::X86Cpuid {
                    eax, ebx, ecx, edx, ..
                } => {
                    for destination in [*eax, *ebx, *ecx, *edx].into_iter().flatten() {
                        invalidate_automatic_aggregate_field(&mut values, &locations, destination);
                    }
                }
                FullInstructionKind::X86Rdtsc { low, high } => {
                    for destination in [*low, *high] {
                        invalidate_automatic_aggregate_field(&mut values, &locations, destination);
                    }
                }
                FullInstructionKind::VaStart { list, .. }
                | FullInstructionKind::VaEnd { list }
                | FullInstructionKind::VaArg { list, .. } => {
                    invalidate_automatic_aggregate_field(&mut values, &locations, *list);
                }
                FullInstructionKind::Constant(_)
                | FullInstructionKind::AddressConstant { .. }
                | FullInstructionKind::AddressOfGlobal { .. }
                | FullInstructionKind::AddressOfFunction { .. }
                | FullInstructionKind::AddressOfString { .. }
                | FullInstructionKind::AddressOfStorage { .. }
                | FullInstructionKind::ProjectField { .. }
                | FullInstructionKind::PointerOffset { .. }
                | FullInstructionKind::PointerDifference { .. }
                | FullInstructionKind::AggregateSnapshot { .. }
                | FullInstructionKind::AggregateProject { .. }
                | FullInstructionKind::Convert { .. }
                | FullInstructionKind::Unary { .. }
                | FullInstructionKind::Binary { .. }
                | FullInstructionKind::IntegerIntrinsic { .. }
                | FullInstructionKind::DirectCall { .. }
                | FullInstructionKind::IndirectCall { .. }
                | FullInstructionKind::RuntimeSize { .. }
                | FullInstructionKind::RuntimePointerOffset { .. }
                | FullInstructionKind::RuntimePointerDifference { .. } => {}
                // New or otherwise unmodeled effects are write barriers.
                _ => values.clear(),
            }
            retained.push(!forwarded);
        }
        retained_instructions.push(retained);
    }

    if aliases.is_empty() {
        return Ok(false);
    }
    for (block, retained) in function.blocks.iter_mut().zip(&retained_instructions) {
        let mut index = 0;
        block.instructions.retain(|_| {
            let retain = retained[index];
            index += 1;
            retain
        });
    }
    remap_promoted_local_update_positions(function, &retained_instructions)?;
    compact_values(function, &aliases)?;
    Ok(true)
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

    let mut used_values = BTreeSet::new();
    for parameter in &function.parameters {
        used_values.extend(parameter.incoming);
    }
    for block in &function.blocks {
        for instruction in &block.instructions {
            used_values.extend(instruction_operands(&instruction.kind));
        }
        if let Some(terminator) = &block.terminator {
            used_values.extend(terminator_operands(terminator));
        }
    }

    let removed = function
        .blocks
        .iter()
        .map(|block| {
            block
                .parameters
                .iter()
                .map(|parameter| {
                    aliases.contains_key(parameter) || !used_values.contains(parameter)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if removed.iter().flatten().all(|removed| !removed) {
        return Ok(false);
    }

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

fn simplify_control_flow(
    types: &TypeStore,
    function: &mut FullFunction,
    config: &EffectiveCompilationConfig,
) -> Result<bool, IrError> {
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
    let value_types = &function.value_types;
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
                .and_then(|constant| {
                    let ty = *value_types.get(selector.0 as usize)?;
                    fold::normalize_integer_constant(types, *constant, ty, config)
                })
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

fn forward_empty_blocks(function: &mut FullFunction) -> Result<bool, IrError> {
    let entry = function.entry;
    let indirect_targets = function
        .blocks
        .iter()
        .filter_map(|block| block.terminator.as_ref())
        .filter_map(|terminator| match terminator {
            FullTerminator::IndirectBranch { targets, .. } => Some(targets),
            _ => None,
        })
        .flatten()
        .map(|edge| edge.target)
        .collect::<BTreeSet<_>>();

    let candidate = function.blocks.iter().find_map(|block| {
        if Some(block.id) == entry
            || !block.instructions.is_empty()
            || !block.parameters.is_empty()
            || indirect_targets.contains(&block.id)
        {
            return None;
        }
        let FullTerminator::Branch(outgoing) = block.terminator.as_ref()? else {
            return None;
        };
        // Thread only parameterless blocks. A live block parameter can
        // dominate uses beyond the outgoing edge; moving such a phi into the
        // successor requires a general CFG rewrite rather than local edge
        // substitution.
        (outgoing.target != block.id)
            .then(|| (block.id, block.parameters.clone(), outgoing.clone()))
    });
    let Some((candidate, parameters, outgoing)) = candidate else {
        return Ok(false);
    };

    for block in &mut function.blocks {
        let Some(terminator) = &mut block.terminator else {
            continue;
        };
        for edge in terminator_edges_mut(terminator) {
            if edge.target != candidate {
                continue;
            }
            if edge.arguments.len() != parameters.len() {
                return Err(IrError::verify(
                    "empty-block predecessor has the wrong argument count",
                ));
            }
            let replacements = parameters
                .iter()
                .copied()
                .zip(edge.arguments.iter().copied())
                .collect::<BTreeMap<_, _>>();
            edge.target = outgoing.target;
            edge.arguments = outgoing
                .arguments
                .iter()
                .map(|argument| replacements.get(argument).copied().unwrap_or(*argument))
                .collect();
        }
    }

    for block in &function.blocks {
        if block.id == candidate {
            continue;
        }
        for instruction in &block.instructions {
            if let Some(parameter) = instruction_operands(&instruction.kind)
                .into_iter()
                .find(|operand| parameters.contains(operand))
            {
                return Err(IrError::verify(format!(
                    "forwarded parameter v{} remains in instruction i{} of b{}",
                    parameter.0, instruction.id.0, block.id.0
                )));
            }
        }
        if let Some(terminator) = &block.terminator
            && let Some(parameter) = terminator_operands(terminator)
                .into_iter()
                .find(|operand| parameters.contains(operand))
        {
            return Err(IrError::verify(format!(
                "forwarded parameter v{} remains in the terminator of b{}",
                parameter.0, block.id.0
            )));
        }
    }

    let retained = function
        .blocks
        .iter()
        .map(|block| block.id != candidate)
        .collect::<Vec<_>>();
    retain_blocks(function, &retained)?;
    compact_values(function, &BTreeMap::new()).map_err(|mut error| {
        error.message = format!(
            "forwarding b{} with parameters {:?} to b{} left invalid values: {}",
            candidate.0, parameters, outgoing.target.0, error.message
        );
        error
    })?;
    Ok(true)
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

    retain_blocks(function, &reachable)?;
    Ok(true)
}

fn retain_blocks(function: &mut FullFunction, retained: &[bool]) -> Result<(), IrError> {
    if retained.len() != function.blocks.len() {
        return Err(IrError::verify(
            "block retention map has the wrong length during optimization",
        ));
    }
    let old_entry = function.entry;
    let mut remap = vec![None; function.blocks.len()];
    let mut next = 0u32;
    for (index, retained) in retained.iter().copied().enumerate() {
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
    function.entry =
        match old_entry {
            Some(entry) => Some(remap[entry.0 as usize].ok_or_else(|| {
                IrError::verify("optimization attempted to remove the entry block")
            })?),
            None => None,
        };
    for local in &mut function.promoted_locals {
        local.updates.retain_mut(|update| {
            let Some(block) = remap.get(update.block.0 as usize).and_then(|block| *block) else {
                return false;
            };
            update.block = block;
            true
        });
    }
    Ok(())
}

fn eliminate_dead_pure_instructions(function: &mut FullFunction) -> Result<bool, IrError> {
    let mut dependencies = vec![Vec::<ValueId>::new(); function.value_types.len()];
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Some(result) = instruction.result {
                dependencies[result.0 as usize].extend(instruction_operands(&instruction.kind));
            }
        }
        if let Some(terminator) = &block.terminator {
            for edge in terminator_edges(terminator) {
                let parameters = &function.blocks[edge.target.0 as usize].parameters;
                for (parameter, argument) in parameters.iter().zip(&edge.arguments) {
                    dependencies[parameter.0 as usize].push(*argument);
                }
            }
        }
    }

    let mut live = vec![false; function.value_types.len()];
    let mut worklist = VecDeque::new();
    let enqueue = |value: ValueId, live: &mut [bool], worklist: &mut VecDeque<ValueId>| {
        if !live[value.0 as usize] {
            live[value.0 as usize] = true;
            worklist.push_back(value);
        }
    };

    // Retain incoming parameter values so ABI and source-debug parameter
    // identities remain stable even when the body does not read them.
    for parameter in &function.parameters {
        if let Some(incoming) = parameter.incoming {
            enqueue(incoming, &mut live, &mut worklist);
        }
    }
    for block in &function.blocks {
        for instruction in &block.instructions {
            if !effects::removable_when_unused(&instruction.kind) {
                for operand in instruction_operands(&instruction.kind) {
                    enqueue(operand, &mut live, &mut worklist);
                }
            }
        }
        if let Some(terminator) = &block.terminator {
            let observed = match terminator {
                FullTerminator::Branch(_) | FullTerminator::Unreachable => Vec::new(),
                FullTerminator::Conditional { condition, .. } => vec![*condition],
                FullTerminator::Switch { selector, .. }
                | FullTerminator::IndirectBranch { selector, .. } => vec![*selector],
                FullTerminator::Return(value) => value.iter().copied().collect(),
            };
            for operand in observed {
                enqueue(operand, &mut live, &mut worklist);
            }
        }
    }
    while let Some(value) = worklist.pop_front() {
        for dependency in dependencies[value.0 as usize].iter().copied() {
            enqueue(dependency, &mut live, &mut worklist);
        }
    }

    let parameter_liveness = function
        .blocks
        .iter()
        .map(|block| {
            block
                .parameters
                .iter()
                .map(|parameter| live[parameter.0 as usize])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut changed = parameter_liveness.iter().flatten().any(|keep| !keep);
    for block in &mut function.blocks {
        if let Some(terminator) = &mut block.terminator {
            for edge in terminator_edges_mut(terminator) {
                let keep = &parameter_liveness[edge.target.0 as usize];
                let mut index = 0;
                edge.arguments.retain(|_| {
                    let retain = keep[index];
                    index += 1;
                    retain
                });
            }
        }
    }
    let retained_instructions = function
        .blocks
        .iter()
        .map(|block| {
            block
                .instructions
                .iter()
                .map(|instruction| {
                    !effects::removable_when_unused(&instruction.kind)
                        || instruction
                            .result
                            .is_some_and(|result| live[result.0 as usize])
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for ((block, keep), retained) in function
        .blocks
        .iter_mut()
        .zip(&parameter_liveness)
        .zip(&retained_instructions)
    {
        let mut index = 0;
        block.parameters.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
        let mut instruction_index = 0;
        block.instructions.retain(|_| {
            let keep = retained[instruction_index];
            instruction_index += 1;
            changed |= !keep;
            keep
        });
    }

    if changed {
        remap_promoted_local_update_positions(function, &retained_instructions)?;
        compact_values(function, &BTreeMap::new())?;
    }
    Ok(changed)
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
