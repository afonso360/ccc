//! Immutable System V AMD64 boundary plans.

mod aarch64;
mod corpus;
mod digest;
mod model;
mod module_plan;
mod riscv64;
mod sysv_amd64;

use std::fmt;

use ccc_session::Span;

use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{TypeId, TypeStore};
pub use corpus::{
    CLASSIFIER_CORPUS_SEED, CorpusAllocationPattern, CorpusBucket, CorpusCase, CorpusFixture,
    CorpusReturnMode, classifier_corpus, selected_cross_link_cases,
};
pub use digest::{
    abi_config_key, ir_shape_digest, sysv_amd64_v1_config_fingerprint, translation_unit_digest,
};
pub use model::*;
pub use module_plan::{dump_module_plan, plan_module};

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

pub fn plan_function_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<NativeBoundaryPlan, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_function_type(types, signature, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_function_type(types, signature, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_function_type(types, signature, config),
    }
}

pub fn plan_boundary_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<BoundaryPlan, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_boundary_type(types, signature, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_boundary_type(types, signature, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_boundary_type(types, signature, config),
    }
}

pub fn plan_variadic_call(
    types: &TypeStore,
    signature: TypeId,
    actual_types: &[TypeId],
    variadic_boundary: usize,
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_variadic_call(
            types,
            signature,
            actual_types,
            variadic_boundary,
            config,
        ),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_variadic_call(types, signature, actual_types, variadic_boundary, config)
        }
        AbiIdentity::RiscvLp64d => {
            riscv64::plan_variadic_call(types, signature, actual_types, variadic_boundary, config)
        }
    }
}

pub fn plan_unprototyped_call(
    types: &TypeStore,
    signature: TypeId,
    promoted_actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => {
            sysv_amd64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
        AbiIdentity::RiscvLp64d => {
            riscv64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
    }
}

pub fn classify_type(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ClassifiedType, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::classify_type(types, ty, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::classify_type(types, ty, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::classify_type(types, ty, config),
    }
}

pub fn plan_va_arg(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<VaArgPlan, AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_va_arg(types, ty, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_va_arg(types, ty, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_va_arg(types, ty, config),
    }
}

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::validate_target(config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => aarch64::validate_target(config),
        AbiIdentity::RiscvLp64d => riscv64::validate_target(config),
    }
}
