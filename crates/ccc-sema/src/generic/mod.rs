//! Semantic analysis for declarations, expressions, and control flow.

mod analyze;
mod dump;
mod model;
mod scopes;

pub use analyze::analyze_frontend;
pub use dump::dump_frontend_typed_ast;
pub use model::*;

#[cfg(test)]
mod tests;
