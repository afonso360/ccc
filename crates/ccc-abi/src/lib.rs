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
use ccc_types::{BuiltinType, FunctionParameters, TypeId, TypeKind, TypeStore};
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
    validate_target(config)?;
    reject_int128_function(types, signature)?;
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
    validate_target(config)?;
    reject_int128_function(types, signature)?;
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
    validate_target(config)?;
    reject_int128_function(types, signature)?;
    for ty in actual_types {
        reject_int128_type(types, *ty, "variadic call argument")?;
    }
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
    validate_target(config)?;
    reject_int128_function(types, signature)?;
    for ty in promoted_actual_types {
        reject_int128_type(types, *ty, "unprototyped call argument")?;
    }
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
    validate_target(config)?;
    reject_int128_type(types, ty, "classified boundary")?;
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
    validate_target(config)?;
    reject_int128_type(types, ty, "`va_arg`")?;
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_va_arg(types, ty, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_va_arg(types, ty, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_va_arg(types, ty, config),
    }
}

fn reject_int128_function(types: &TypeStore, signature: TypeId) -> Result<(), AbiError> {
    let signature = types.function_signature(signature).ok_or_else(|| {
        AbiError::new(
            "CCC3501",
            format!("type `{}` is not a function type", types.display(signature)),
        )
    })?;
    reject_int128_type(types, signature.result.ty, "function return")?;
    if let FunctionParameters::Prototype(parameters) = signature.parameters {
        for parameter in parameters {
            reject_int128_type(types, parameter.ty, "function parameter")?;
        }
    }
    Ok(())
}

fn reject_int128_type(types: &TypeStore, ty: TypeId, boundary: &str) -> Result<(), AbiError> {
    fn contains(types: &TypeStore, ty: TypeId, active: &mut Vec<TypeId>) -> Result<bool, AbiError> {
        if matches!(
            types.builtin_type(ty),
            Some(BuiltinType::Int128 | BuiltinType::UnsignedInt128)
        ) {
            return Ok(true);
        }
        if active.contains(&ty) {
            return Ok(false);
        }
        active.push(ty);
        let result = match types.try_kind(ty) {
            Some(TypeKind::Array(array)) => contains(types, array.element.ty, active)?,
            Some(TypeKind::Record(id)) => {
                let fields = types
                    .record(*id)
                    .and_then(|record| record.fields.as_ref())
                    .ok_or_else(|| {
                        AbiError::new(
                            "CCC3502",
                            format!("type `{}` is incomplete", types.display(ty)),
                        )
                    })?;
                let mut found = false;
                for field in fields {
                    found |= contains(types, field.ty.ty, active)?;
                }
                found
            }
            _ => false,
        };
        active.pop();
        Ok(result)
    }

    if contains(types, ty, &mut Vec::new())? {
        return Err(AbiError::new(
            "CCC3517",
            format!(
                "{boundary} type `{}` contains a 128-bit integer with no enabled ABI transport",
                types.display(ty)
            ),
        ));
    }
    Ok(())
}

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::validate_target(config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => aarch64::validate_target(config),
        AbiIdentity::RiscvLp64d => riscv64::validate_target(config),
    }
}

#[cfg(test)]
mod int128_tests {
    use ccc_target::EffectiveCompilationConfig;
    use ccc_types::{Field, FunctionType, QualifiedType, RecordKind, TypeId, TypeStore};

    use super::{plan_function_type, plan_va_arg};

    #[test]
    fn scalar_and_aggregate_128_bit_boundaries_fail_before_target_classification() {
        for config in [
            EffectiveCompilationConfig::default(),
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            let mut types = TypeStore::default();
            let (record, wrapper) = types.declare_record(RecordKind::Struct, None);
            types
                .complete_record(record, vec![Field::named("value", TypeId::INT128)])
                .unwrap();
            for ty in [TypeId::INT128, TypeId::UNSIGNED_INT128, wrapper] {
                let signature = types.function_type(FunctionType::prototype(
                    QualifiedType::unqualified(ty),
                    vec![QualifiedType::unqualified(ty)],
                ));
                let error = plan_function_type(&types, signature, &config).unwrap_err();
                assert_eq!(error.code, "CCC3517", "{}", config.target.triple);
                assert!(error.message.contains("128-bit integer"), "{error}");

                let error = plan_va_arg(&types, ty, &config).unwrap_err();
                assert_eq!(error.code, "CCC3517", "{}", config.target.triple);
                assert!(error.message.contains("128-bit integer"), "{error}");
            }
        }
    }
}
