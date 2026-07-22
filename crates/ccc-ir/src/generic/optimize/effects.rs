use super::super::{FullInstructionKind, MemoryAccess};

/// Whether an instruction with no observable result can be removed without
/// changing the C abstract-machine behavior.
///
/// Keep this match exhaustive. New IR operations must make an explicit effect
/// decision before the optimizer can compile against them.
pub(super) fn removable_when_unused(kind: &FullInstructionKind) -> bool {
    match kind {
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
        | FullInstructionKind::IntegerIntrinsic { .. } => true,

        FullInstructionKind::Load { access, .. }
        | FullInstructionKind::BitfieldLoad { access, .. } => !ordered_access(*access),

        FullInstructionKind::Store { .. }
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
        | FullInstructionKind::RuntimePointerDifference { .. } => false,
    }
}

const fn ordered_access(access: MemoryAccess) -> bool {
    access.volatile || access.atomic.is_some() || access.non_elidable || access.non_movable
}
