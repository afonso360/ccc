//! Immutable System V AMD64 boundary plans.

mod corpus;
mod digest;
mod model;
mod module_plan;
mod sysv_amd64;

use std::fmt;

use ccc_session::Span;

pub use corpus::{
    CLASSIFIER_CORPUS_SEED, CorpusAllocationPattern, CorpusBucket, CorpusCase, CorpusFixture,
    CorpusReturnMode, classifier_corpus, selected_cross_link_cases,
};
pub use digest::{abi_config_key, ir_shape_digest, translation_unit_digest};
pub use model::*;
pub use module_plan::{dump_module_plan, plan_module};
pub use sysv_amd64::{
    classify_type, plan_boundary_type, plan_function_type, plan_unprototyped_call, plan_va_arg,
    plan_variadic_call,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiError {
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl AbiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: None,
        }
    }

    pub fn with_span_if_none(mut self, span: Span) -> Self {
        self.span.get_or_insert(span);
        self
    }
}

impl fmt::Display for AbiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for AbiError {}
