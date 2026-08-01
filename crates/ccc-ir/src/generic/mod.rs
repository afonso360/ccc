//! Typed control-flow IR for the complete C frontend.

mod dump;
mod lower;
mod model;
mod optimize;
mod verify;

pub use dump::dump_frontend_ir;
pub use lower::lower_frontend;
pub use model::*;
pub use optimize::{optimize_frontend, optimize_frontend_for_config};
pub use verify::verify_frontend;

/// Counts each SSA value use in instruction operands and control-flow edges.
///
/// The count is saturated because consumers only need to distinguish unused,
/// single-use, and multi-use values.
pub fn value_use_counts(function: &FullFunction) -> Vec<usize> {
    let mut counts = vec![0usize; function.value_types.len()];
    for block in &function.blocks {
        for instruction in &block.instructions {
            for value in verify::instruction_operands(&instruction.kind) {
                let count = &mut counts[value.0 as usize];
                *count = count.saturating_add(1);
            }
        }
        if let Some(terminator) = &block.terminator {
            for value in verify::terminator_operands(terminator) {
                let count = &mut counts[value.0 as usize];
                *count = count.saturating_add(1);
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests;
