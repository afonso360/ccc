//! Typed control-flow IR for the complete C frontend.

mod dump;
mod lower;
mod model;
mod optimize;
mod verify;

pub use dump::dump_frontend_ir;
pub use lower::lower_frontend;
pub use model::*;
pub use optimize::optimize_frontend;
pub use verify::verify_frontend;

#[cfg(test)]
mod tests;
