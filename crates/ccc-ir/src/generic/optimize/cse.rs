use std::collections::BTreeMap;

use super::super::{FullFunction, FullInstructionKind, IrError, ScalarConstant, ValueId};
use super::resolve_alias;
use crate::generic::lower::compact_values;

/// Eliminates repeated scalar and address expressions within one basic block.
///
/// The scope is intentionally local: this avoids reconstructing dominance and
/// memory-version information that Cranelift already owns. CCC performs this
/// subset because it makes its verifier-backed IR and pre-ABI dump canonical.
pub(super) fn eliminate_common_expressions(function: &mut FullFunction) -> Result<bool, IrError> {
    let mut aliases = BTreeMap::<ValueId, ValueId>::new();
    let value_types = function.value_types.clone();
    for block in &mut function.blocks {
        let mut available = BTreeMap::<String, ValueId>::new();
        let mut retained = Vec::with_capacity(block.instructions.len());
        for instruction in std::mem::take(&mut block.instructions) {
            let Some(result) = instruction.result else {
                retained.push(instruction);
                continue;
            };
            let Some(mut key) = expression_key(&instruction.kind, &aliases)? else {
                retained.push(instruction);
                continue;
            };
            let result_ty = value_types
                .get(result.0 as usize)
                .ok_or_else(|| IrError::verify("CSE result has no SSA type"))?;
            key.push_str(&format!("->{}", result_ty.index()));
            if let Some(existing) = available.get(&key).copied() {
                aliases.insert(result, existing);
            } else {
                available.insert(key, result);
                retained.push(instruction);
            }
        }
        block.instructions = retained;
    }
    if aliases.is_empty() {
        return Ok(false);
    }
    compact_values(function, &aliases)?;
    Ok(true)
}

fn expression_key(
    kind: &FullInstructionKind,
    aliases: &BTreeMap<ValueId, ValueId>,
) -> Result<Option<String>, IrError> {
    let value = |id: ValueId| -> Result<u32, IrError> { Ok(resolve_alias(id, aliases)?.0) };
    let key = match kind {
        FullInstructionKind::Constant(ScalarConstant::Signed(constant)) => {
            format!("constant:signed:{constant}")
        }
        FullInstructionKind::Constant(ScalarConstant::Unsigned(constant)) => {
            format!("constant:unsigned:{constant}")
        }
        FullInstructionKind::Constant(ScalarConstant::NullPointer) => "constant:null".to_owned(),
        FullInstructionKind::Constant(
            ScalarConstant::Floating(_) | ScalarConstant::LongDouble(_),
        ) => return Ok(None),
        FullInstructionKind::AddressConstant {
            target,
            addend,
            one_past,
        } => format!("address.constant:{target:?}:{addend}:{one_past}"),
        FullInstructionKind::AddressOfGlobal { global } => format!("address.global:{}", global.0),
        FullInstructionKind::AddressOfFunction {
            function,
            signature,
        } => format!("address.function:{}:{}", function.0, signature.index()),
        FullInstructionKind::AddressOfString { string } => format!("address.string:{}", string.0),
        // The current address of runtime-sized storage may change at a later
        // allocation point. Keep every storage-address observation explicit
        // rather than teaching this local pass about allocation epochs.
        FullInstructionKind::AddressOfStorage { .. } => return Ok(None),
        FullInstructionKind::ProjectField {
            base,
            record,
            field_index,
            field_name,
        } => format!(
            "project.field:{}:{}:{:?}:{field_index}:{field_name:?}",
            value(*base)?,
            record.ty.index(),
            record.qualifiers
        ),
        FullInstructionKind::PointerOffset {
            base,
            index,
            element,
            subtract,
        } => format!(
            "pointer.offset:{}:{}:{}:{:?}:{subtract}",
            value(*base)?,
            value(*index)?,
            element.ty.index(),
            element.qualifiers
        ),
        FullInstructionKind::PointerDifference {
            left,
            right,
            element,
        } => format!(
            "pointer.difference:{}:{}:{}:{:?}",
            value(*left)?,
            value(*right)?,
            element.ty.index(),
            element.qualifiers
        ),
        FullInstructionKind::Convert {
            kind,
            operand,
            from,
            to,
        } => format!(
            "convert:{kind:?}:{}:{}:{:?}:{}:{:?}",
            value(*operand)?,
            from.ty.index(),
            from.qualifiers,
            to.ty.index(),
            to.qualifiers
        ),
        FullInstructionKind::Unary { operator, operand } => {
            format!("unary:{operator:?}:{}", value(*operand)?)
        }
        FullInstructionKind::Binary {
            operator,
            left,
            right,
        } => format!("binary:{operator:?}:{}:{}", value(*left)?, value(*right)?),
        FullInstructionKind::IntegerIntrinsic { operation, operand } => {
            format!("integer.intrinsic:{operation:?}:{}", value(*operand)?)
        }
        FullInstructionKind::AggregateProject { .. }
        | FullInstructionKind::Load { .. }
        | FullInstructionKind::Store { .. }
        | FullInstructionKind::BitfieldLoad { .. }
        | FullInstructionKind::BitfieldStore { .. }
        | FullInstructionKind::ZeroInitialize { .. }
        | FullInstructionKind::StringInitialize { .. }
        | FullInstructionKind::AggregateCopy { .. }
        | FullInstructionKind::AggregateSnapshot { .. }
        | FullInstructionKind::MemoryCopy { .. }
        | FullInstructionKind::MemorySet { .. }
        | FullInstructionKind::DirectCall { .. }
        | FullInstructionKind::IndirectCall { .. }
        | FullInstructionKind::AtomicReadModifyWrite { .. }
        | FullInstructionKind::AtomicCompareExchange { .. }
        | FullInstructionKind::Prefetch { .. }
        | FullInstructionKind::MemoryFence { .. }
        | FullInstructionKind::CompilerBarrier { .. }
        | FullInstructionKind::OpaqueScalar { .. }
        | FullInstructionKind::CodeLayoutHint(_)
        | FullInstructionKind::X86Cpuid { .. }
        | FullInstructionKind::X86Rdtsc { .. }
        | FullInstructionKind::VaStart { .. }
        | FullInstructionKind::VaArg { .. }
        | FullInstructionKind::VaCopy { .. }
        | FullInstructionKind::VaEnd { .. }
        | FullInstructionKind::RuntimeSize { .. }
        | FullInstructionKind::RuntimeSizedAllocate { .. }
        | FullInstructionKind::RuntimePointerOffset { .. }
        | FullInstructionKind::RuntimePointerDifference { .. } => return Ok(None),
    };
    Ok(Some(key))
}
