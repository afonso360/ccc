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
    reject_int128_function(types, signature, config)?;
    reject_float16_function(types, signature)?;
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
    reject_int128_function(types, signature, config)?;
    reject_float16_function(types, signature)?;
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
    reject_int128_function(types, signature, config)?;
    reject_float16_function(types, signature)?;
    for ty in actual_types {
        reject_int128_type(types, *ty, "variadic call argument", config)?;
        reject_float16_type(types, *ty, "variadic call argument")?;
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

pub fn plan_fixed_call(
    types: &TypeStore,
    signature: TypeId,
    actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    reject_int128_function(types, signature, config)?;
    reject_float16_function(types, signature)?;
    for ty in actual_types {
        reject_int128_type(types, *ty, "fixed call argument", config)?;
        reject_float16_type(types, *ty, "fixed call argument")?;
    }
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => {
            sysv_amd64::plan_fixed_call(types, signature, actual_types, config)
        }
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 | AbiIdentity::RiscvLp64d => {
            Err(AbiError::new(
                "CCC3511",
                "the selected target has no fixed wide-integer call bridge",
            ))
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
    reject_int128_function(types, signature, config)?;
    reject_float16_function(types, signature)?;
    for ty in promoted_actual_types {
        reject_int128_type(types, *ty, "unprototyped call argument", config)?;
        reject_float16_type(types, *ty, "unprototyped call argument")?;
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
    reject_int128_type(types, ty, "classified boundary", config)?;
    reject_float16_type(types, ty, "classified boundary")?;
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
    reject_int128_type(types, ty, "`va_arg`", config)?;
    reject_float16_type(types, ty, "`va_arg`")?;
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_va_arg(types, ty, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_va_arg(types, ty, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_va_arg(types, ty, config),
    }
}

fn reject_int128_function(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<(), AbiError> {
    let signature = types.function_signature(signature).ok_or_else(|| {
        AbiError::new(
            "CCC3501",
            format!("type `{}` is not a function type", types.display(signature)),
        )
    })?;
    reject_int128_type(types, signature.result.ty, "function return", config)?;
    if let FunctionParameters::Prototype(parameters) = signature.parameters {
        for parameter in parameters {
            reject_int128_type(types, parameter.ty, "function parameter", config)?;
        }
    }
    Ok(())
}

fn reject_float16_function(types: &TypeStore, signature: TypeId) -> Result<(), AbiError> {
    let signature = types.function_signature(signature).ok_or_else(|| {
        AbiError::new(
            "CCC3501",
            format!("type `{}` is not a function type", types.display(signature)),
        )
    })?;
    reject_float16_type(types, signature.result.ty, "function return")?;
    if let FunctionParameters::Prototype(parameters) = signature.parameters {
        for parameter in parameters {
            reject_float16_type(types, parameter.ty, "function parameter")?;
        }
    }
    Ok(())
}

fn reject_int128_type(
    types: &TypeStore,
    ty: TypeId,
    boundary: &str,
    config: &EffectiveCompilationConfig,
) -> Result<(), AbiError> {
    if config.target.abi.supports_int128_values() {
        return Ok(());
    }
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

fn reject_float16_type(types: &TypeStore, ty: TypeId, boundary: &str) -> Result<(), AbiError> {
    fn contains(types: &TypeStore, ty: TypeId, active: &mut Vec<TypeId>) -> Result<bool, AbiError> {
        if types.builtin_type(ty) == Some(BuiltinType::Float16) {
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
            "CCC3518",
            format!(
                "{boundary} type `{}` contains `_Float16` with no enabled ABI transport",
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

    use super::{BoundaryPlan, classify_type, plan_boundary_type, plan_function_type, plan_va_arg};

    #[test]
    fn scalar_and_aggregate_128_bit_boundaries_are_enabled_only_on_sysv_amd64() {
        let mut types = TypeStore::default();
        let (record, wrapper) = types.declare_record(RecordKind::Struct, None);
        types
            .complete_record(record, vec![Field::named("value", TypeId::INT128)])
            .unwrap();

        let config = EffectiveCompilationConfig::default();
        for ty in [TypeId::INT128, TypeId::UNSIGNED_INT128, wrapper] {
            let classified = classify_type(&types, ty, &config).unwrap();
            assert_eq!((classified.size, classified.align), (16, 16));
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(ty),
                vec![QualifiedType::unqualified(ty)],
            ));
            assert!(matches!(
                plan_boundary_type(&types, signature, &config).unwrap(),
                BoundaryPlan::Bridge(_)
            ));
            assert_eq!(
                plan_function_type(&types, signature, &config)
                    .unwrap_err()
                    .code,
                "CCC3510"
            );
            let va_arg = plan_va_arg(&types, ty, &config).unwrap();
            assert_eq!((va_arg.overflow_size, va_arg.overflow_align), (16, 16));
        }

        for config in [
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            for ty in [TypeId::INT128, TypeId::UNSIGNED_INT128, wrapper] {
                let signature = types.function_type(FunctionType::prototype(
                    QualifiedType::unqualified(ty),
                    vec![QualifiedType::unqualified(ty)],
                ));
                for error in [
                    classify_type(&types, ty, &config).unwrap_err(),
                    plan_boundary_type(&types, signature, &config).unwrap_err(),
                    plan_va_arg(&types, ty, &config).unwrap_err(),
                ] {
                    assert_eq!(error.code, "CCC3517", "{}", config.target.triple);
                    assert!(error.message.contains("128-bit integer"), "{error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod float16_tests {
    use ccc_target::EffectiveCompilationConfig;
    use ccc_types::{Field, FunctionType, QualifiedType, RecordKind, TypeId, TypeStore};

    use super::{plan_function_type, plan_va_arg};

    #[test]
    fn scalar_and_aggregate_float16_boundaries_fail_before_target_classification() {
        for config in [
            EffectiveCompilationConfig::default(),
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            let mut types = TypeStore::default();
            let (record, wrapper) = types.declare_record(RecordKind::Struct, None);
            types
                .complete_record(record, vec![Field::named("value", TypeId::FLOAT16)])
                .unwrap();
            for ty in [TypeId::FLOAT16, wrapper] {
                let signature = types.function_type(FunctionType::prototype(
                    QualifiedType::unqualified(ty),
                    vec![QualifiedType::unqualified(ty)],
                ));
                let error = plan_function_type(&types, signature, &config).unwrap_err();
                assert_eq!(error.code, "CCC3518", "{}", config.target.triple);
                assert!(error.message.contains("_Float16"), "{error}");

                let error = plan_va_arg(&types, ty, &config).unwrap_err();
                assert_eq!(error.code, "CCC3518", "{}", config.target.triple);
                assert!(error.message.contains("_Float16"), "{error}");
            }
        }
    }
}
