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
    let boundary = match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::plan_boundary_type(types, signature, config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_boundary_type(types, signature, config)
        }
        AbiIdentity::RiscvLp64d => riscv64::plan_boundary_type(types, signature, config),
    }?;
    validate_boundary_register_high_water(boundary)
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
    for ty in actual_types {
        reject_int128_type(types, *ty, "variadic call argument", config)?;
    }
    let plan = match config.target.abi {
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
    }?;
    validate_bridge_register_high_water(plan)
}

pub fn plan_fixed_call(
    types: &TypeStore,
    signature: TypeId,
    actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    reject_int128_function(types, signature, config)?;
    for ty in actual_types {
        reject_int128_type(types, *ty, "fixed call argument", config)?;
    }
    let plan = match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => {
            sysv_amd64::plan_fixed_call(types, signature, actual_types, config)
        }
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 | AbiIdentity::RiscvLp64d => {
            Err(AbiError::new(
                "CCC3511",
                "the selected target has no fixed wide-integer call bridge",
            ))
        }
    }?;
    validate_bridge_register_high_water(plan)
}

pub fn plan_unprototyped_call(
    types: &TypeStore,
    signature: TypeId,
    promoted_actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    reject_int128_function(types, signature, config)?;
    for ty in promoted_actual_types {
        reject_int128_type(types, *ty, "unprototyped call argument", config)?;
    }
    let plan = match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => {
            sysv_amd64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            aarch64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
        AbiIdentity::RiscvLp64d => {
            riscv64::plan_unprototyped_call(types, signature, promoted_actual_types, config)
        }
    }?;
    validate_bridge_register_high_water(plan)
}

fn validate_boundary_register_high_water(boundary: BoundaryPlan) -> Result<BoundaryPlan, AbiError> {
    match boundary {
        BoundaryPlan::Bridge(plan) => {
            validate_bridge_register_high_water(plan).map(BoundaryPlan::Bridge)
        }
        BoundaryPlan::Native(plan) => Ok(BoundaryPlan::Native(plan)),
    }
}

fn validate_bridge_register_high_water(
    plan: BridgeBoundaryPlan,
) -> Result<BridgeBoundaryPlan, AbiError> {
    let gp_capacity = if plan.abi_identity == AbiIdentity::SysvAmd64Lp64 {
        6
    } else {
        8
    };
    let fp_capacity = 8;
    if plan.gp_used > gp_capacity || plan.xmm_used > fp_capacity {
        return Err(AbiError::new(
            "CCC3515",
            format!(
                "bridge register high-water counts gp={} fp={} exceed {} ABI capacities gp={gp_capacity} fp={fp_capacity}",
                plan.gp_used,
                plan.xmm_used,
                plan.abi_identity.name()
            ),
        ));
    }
    if plan.variadic_sse_count > plan.xmm_used || plan.variadic_sse_count > fp_capacity {
        return Err(AbiError::new(
            "CCC3515",
            "bridge variadic floating-register count exceeds its live prefix",
        ));
    }
    for piece in &plan.parameter_pieces {
        let BridgeLocation::Register(register) = piece.location else {
            continue;
        };
        let within_prefix = match register.bank {
            RegisterBank::Integer => register.index < plan.gp_used,
            RegisterBank::Float => register.index < plan.xmm_used,
            RegisterBank::X87 => false,
        };
        if !within_prefix {
            return Err(AbiError::new(
                "CCC3515",
                format!(
                    "bridge parameter register {:?} lies outside the declared gp={} fp={} live prefixes",
                    register, plan.gp_used, plan.xmm_used
                ),
            ));
        }
    }
    Ok(plan)
}

pub fn classify_type(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ClassifiedType, AbiError> {
    validate_target(config)?;
    reject_int128_type(types, ty, "classified boundary", config)?;
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

pub(crate) fn boundary_value_alignment(
    types: &TypeStore,
    classified: &ClassifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<u64, AbiError> {
    if classified.passing == PassingMode::Scalar
        && let Some(TypeKind::AlignmentAdjusted(adjusted)) = types.try_kind(classified.ty)
    {
        return types
            .layout_of(adjusted.underlying, config)
            .map(|layout| layout.align)
            .map_err(|error| {
                AbiError::new(
                    "CCC3502",
                    format!("scalar has no underlying ABI layout: {error}"),
                )
            });
    }
    Ok(classified.align)
}

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    match config.target.abi {
        AbiIdentity::SysvAmd64Lp64 => sysv_amd64::validate_target(config),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => aarch64::validate_target(config),
        AbiIdentity::RiscvLp64d => riscv64::validate_target(config),
    }
}

#[cfg(test)]
mod bridge_high_water_tests {
    use ccc_target::{AbiIdentity, enabled_compilation_configs};
    use ccc_types::{FunctionType, QualifiedType, TypeId, TypeStore};

    use super::{plan_variadic_call, validate_bridge_register_high_water};

    #[test]
    fn bridge_register_high_water_counts_are_bounded_on_every_target() {
        let mut types = TypeStore::default();
        let signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::INT),
            vec![QualifiedType::unqualified(TypeId::INT)],
        ));
        for config in enabled_compilation_configs() {
            let plan = plan_variadic_call(&types, signature, &[TypeId::INT], 1, &config).unwrap();
            let gp_capacity = if config.target.abi == AbiIdentity::SysvAmd64Lp64 {
                6
            } else {
                8
            };
            assert!(plan.gp_used <= gp_capacity);
            assert!(plan.xmm_used <= 8);

            let mut excessive_gp = plan.clone();
            excessive_gp.gp_used = gp_capacity + 1;
            assert_eq!(
                validate_bridge_register_high_water(excessive_gp)
                    .unwrap_err()
                    .code,
                "CCC3515"
            );

            let mut excessive_fp = plan;
            excessive_fp.xmm_used = 9;
            assert_eq!(
                validate_bridge_register_high_water(excessive_fp)
                    .unwrap_err()
                    .code,
                "CCC3515"
            );
        }
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
    use ccc_target::enabled_compilation_configs;
    use ccc_types::{Field, FunctionType, QualifiedType, RecordKind, TypeId, TypeStore};

    use super::{AbiCarrier, AbiClass, PassingMode, plan_function_type, plan_va_arg};

    #[test]
    fn scalar_and_aggregate_float16_boundaries_are_classified_per_target() {
        for config in enabled_compilation_configs() {
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
                let function = plan_function_type(&types, signature, &config)
                    .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
                let va_arg = plan_va_arg(&types, ty, &config)
                    .unwrap_or_else(|error| panic!("{}: {error}", config.target.triple));
                if ty == TypeId::FLOAT16 {
                    assert_eq!(
                        function.parameters[0].classified.passing,
                        PassingMode::Scalar
                    );
                    assert_eq!(function.parameters[0].classified.classes, [AbiClass::Sse]);
                    assert_eq!(function.clif_parameters[0].carrier, AbiCarrier::F16);
                    assert_eq!(function.clif_results[0].carrier, AbiCarrier::F16);
                    assert_eq!(va_arg.classified.passing, PassingMode::Scalar);
                }
            }
        }
    }
}

#[cfg(test)]
mod alignment_adjusted_tests {
    use ccc_target::{AbiIdentity, EffectiveCompilationConfig, enabled_compilation_configs};
    use ccc_types::{
        Field, FunctionParameters, FunctionType, QualifiedType, RecordKind, TypeId, TypeStore,
    };

    use super::{
        AbiClass, BridgeLocation, PassingMode, RegisterSlot, classify_type, plan_unprototyped_call,
        plan_va_arg,
    };

    #[test]
    fn integer_alignment_adjustments_preserve_scalar_carriers_on_every_target() {
        let mut types = TypeStore::default();
        let adjusted = types.alignment_adjusted(TypeId::UNSIGNED_INT, 1);
        let (record_id, record) = types.declare_record(RecordKind::Struct, None);
        types
            .complete_record(
                record_id,
                vec![
                    Field::named("tag", TypeId::CHAR),
                    Field::named("value", adjusted),
                ],
            )
            .unwrap();

        for config in enabled_compilation_configs() {
            let scalar = classify_type(&types, adjusted, &config).unwrap();
            assert_eq!(
                (scalar.size, scalar.align),
                (4, 1),
                "{}",
                config.target.triple
            );
            assert_eq!(
                scalar.passing,
                PassingMode::Scalar,
                "{}",
                config.target.triple
            );
            assert_eq!(
                scalar.classes,
                [AbiClass::Integer],
                "{}",
                config.target.triple
            );

            let aggregate = classify_type(&types, record, &config).unwrap();
            assert_eq!(
                (aggregate.size, aggregate.align),
                (5, 1),
                "{}",
                config.target.triple
            );
            if config.target.abi == AbiIdentity::SysvAmd64Lp64 {
                assert_eq!(
                    aggregate.passing,
                    PassingMode::Memory,
                    "{}",
                    config.target.triple
                );
            } else {
                assert_ne!(
                    aggregate.passing,
                    PassingMode::Memory,
                    "{}",
                    config.target.triple
                );
            }
        }
    }

    #[test]
    fn scalar_boundary_and_variadic_alignment_follow_each_target_abi() {
        let mut types = TypeStore::default();
        let under_aligned = types.alignment_adjusted(TypeId::UNSIGNED_INT, 1);
        let over_aligned = types.alignment_adjusted(TypeId::UNSIGNED_INT, 16);
        let signature = types.function_type(FunctionType {
            result: QualifiedType::unqualified(TypeId::VOID),
            parameters: FunctionParameters::Unspecified,
            variadic: false,
        });

        for (config, register_capacity, overflow_align, stack_offset, register) in [
            (EffectiveCompilationConfig::default(), 6usize, 8, 8, Some(1)),
            (
                EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
                8,
                8,
                8,
                Some(1),
            ),
            (
                EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
                8,
                8,
                8,
                Some(1),
            ),
            (
                EffectiveCompilationConfig::aarch64_apple_darwin(),
                0,
                8,
                8,
                None,
            ),
        ] {
            let va_arg = plan_va_arg(&types, under_aligned, &config).unwrap();
            assert_eq!(va_arg.result_align, 1, "{}", config.target.triple);
            assert_eq!(va_arg.overflow_align, 8, "{}", config.target.triple);
            let va_arg = plan_va_arg(&types, over_aligned, &config).unwrap();
            assert_eq!(va_arg.result_align, 16, "{}", config.target.triple);
            assert_eq!(
                va_arg.overflow_align, overflow_align,
                "{}",
                config.target.triple
            );

            let mut actual = vec![TypeId::UNSIGNED_INT; register_capacity + 2];
            actual[register_capacity + 1] = over_aligned;
            let plan = plan_unprototyped_call(&types, signature, &actual, &config).unwrap();
            let location = |source_index| {
                plan.parameter_pieces
                    .iter()
                    .find(|piece| piece.source_index == Some(source_index as u32))
                    .unwrap()
                    .location
            };
            assert_eq!(
                location(register_capacity),
                BridgeLocation::Stack { offset: 0 },
                "{}",
                config.target.triple
            );
            assert_eq!(
                location(register_capacity + 1),
                BridgeLocation::Stack {
                    offset: stack_offset,
                },
                "{}",
                config.target.triple
            );

            if let Some(register) = register {
                let plan = plan_unprototyped_call(
                    &types,
                    signature,
                    &[TypeId::UNSIGNED_INT, over_aligned],
                    &config,
                )
                .unwrap();
                let adjusted = plan
                    .parameter_pieces
                    .iter()
                    .find(|piece| piece.source_index == Some(1))
                    .unwrap();
                assert_eq!(
                    adjusted.location,
                    BridgeLocation::Register(RegisterSlot::integer(register)),
                    "{}",
                    config.target.triple
                );
            }
        }
    }
}
