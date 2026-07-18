//! AAPCS64 and Darwin arm64 fixed-boundary classification.

use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, FunctionType, LayoutShape, RecordKind, TypeId,
    TypeKind, TypeStore,
};

use crate::{AbiError, model::*};

const MAX_ARGUMENT_REGISTERS: u8 = 8;
const MAX_RESULT_REGISTERS: usize = 8;

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    if !matches!(
        config.target.abi,
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64
    ) {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not use an enabled arm64 ABI identity",
                config.target.triple
            ),
        ));
    }
    let layout = config.target.data_layout;
    if layout.pointer_width != 64
        || layout.int_width != 32
        || layout.long_width != 64
        || layout.double_width != 64
    {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not satisfy the enabled arm64 LP64 data model",
                config.target.triple
            ),
        ));
    }
    Ok(())
}

pub(crate) fn plan_function_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<NativeBoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if signature.variadic {
        return Err(variadic_transport_error(config, "function type"));
    }
    plan_native_signature(types, &signature, config)
}

pub(crate) fn plan_boundary_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<BoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if signature.variadic {
        return Err(variadic_transport_error(config, "function definition"));
    }
    Ok(BoundaryPlan::Native(plan_native_signature(
        types, &signature, config,
    )?))
}

pub(crate) fn plan_variadic_call(
    _types: &TypeStore,
    _signature: TypeId,
    _actual_types: &[TypeId],
    _variadic_boundary: usize,
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error(config, "variadic call"))
}

pub(crate) fn plan_unprototyped_call(
    _types: &TypeStore,
    _signature: TypeId,
    _promoted_actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error(config, "unprototyped call"))
}

pub(crate) fn classify_type(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ClassifiedType, AbiError> {
    validate_target(config)?;
    classify(types, ty, config, "boundary")
}

pub(crate) fn plan_va_arg(
    _types: &TypeStore,
    _ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<VaArgPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error(config, "va_arg"))
}

fn variadic_transport_error(config: &EffectiveCompilationConfig, boundary: &str) -> AbiError {
    AbiError::new(
        "CCC3520",
        format!(
            "{boundary} requires the {} variadic transport adapter",
            config.target.abi.name()
        ),
    )
}

fn function_signature(types: &TypeStore, signature: TypeId) -> Result<FunctionType, AbiError> {
    types.function_signature(signature).ok_or_else(|| {
        AbiError::new(
            "CCC3505",
            format!(
                "type `{}` is not a function type and has no function ABI plan",
                types.display(signature)
            ),
        )
    })
}

fn plan_native_signature(
    types: &TypeStore,
    signature: &FunctionType,
    config: &EffectiveCompilationConfig,
) -> Result<NativeBoundaryPlan, AbiError> {
    let FunctionParameters::Prototype(parameters) = &signature.parameters else {
        return Err(AbiError::new(
            "CCC3506",
            "a function type without a prototype has no fixed ABI plan",
        ));
    };

    let result_classified = classify(types, signature.result.ty, config, "return")?;
    reject_array_boundary(types, signature.result.ty, "return")?;
    let mut clif_parameters = Vec::new();
    let indirect_result = result_classified.passing == PassingMode::Memory;
    if indirect_result {
        push_carrier(
            &mut clif_parameters,
            None,
            None,
            0,
            8,
            AbiClass::Integer,
            AbiCarrier::I64,
            IntegerExtension::None,
            NativePurpose::StructReturn,
        )?;
    }

    let mut gp_used = 0u8;
    let mut fp_used = 0u8;
    let mut planned_parameters = Vec::with_capacity(parameters.len());
    for (source_index, parameter) in parameters.iter().enumerate() {
        reject_array_boundary(types, parameter.ty, "parameter")?;
        let classified = classify(types, parameter.ty, config, "parameter")?;
        if classified.passing == PassingMode::Void {
            return Err(AbiError::new(
                "CCC3507",
                "`void` cannot appear as a function parameter type",
            ));
        }
        let source_index = source_index as u32;
        let mut carrier_indices = Vec::new();
        match classified.passing {
            PassingMode::Scalar => {
                let scalar = boundary_scalar(types, parameter.ty, config, "parameter")?;
                let class = scalar_class(scalar);
                if class == AbiClass::Integer {
                    gp_used = gp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                } else {
                    fp_used = fp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                }
                carrier_indices.push(push_carrier(
                    &mut clif_parameters,
                    Some(source_index),
                    None,
                    0,
                    scalar_size(scalar),
                    class,
                    scalar_carrier(scalar),
                    scalar_extension(scalar),
                    NativePurpose::Normal,
                )?);
            }
            PassingMode::Registers if is_homogeneous(&classified) => {
                let pieces = classified.pieces.len() as u8;
                if fp_used < MAX_ARGUMENT_REGISTERS
                    && fp_used.saturating_add(pieces) > MAX_ARGUMENT_REGISTERS
                {
                    return Err(AbiError::new(
                        "CCC3521",
                        format!(
                            "parameter type `{}` is a homogeneous aggregate that straddles the arm64 FP-register boundary",
                            types.display(parameter.ty)
                        ),
                    ));
                }
                fp_used = fp_used.saturating_add(pieces).min(MAX_ARGUMENT_REGISTERS);
                for piece in &classified.pieces {
                    carrier_indices.push(push_carrier(
                        &mut clif_parameters,
                        Some(source_index),
                        Some(piece.index),
                        piece.offset,
                        piece.valid_bytes,
                        piece.class,
                        float_piece_carrier(piece)?,
                        IntegerExtension::None,
                        NativePurpose::Normal,
                    )?);
                }
            }
            PassingMode::Registers => {
                if classified.align >= 16
                    && gp_used < MAX_ARGUMENT_REGISTERS
                    && !gp_used.is_multiple_of(2)
                {
                    push_carrier(
                        &mut clif_parameters,
                        None,
                        None,
                        0,
                        8,
                        AbiClass::Integer,
                        AbiCarrier::I64,
                        IntegerExtension::None,
                        NativePurpose::Padding,
                    )?;
                    gp_used += 1;
                }
                gp_used = gp_used
                    .saturating_add(classified.pieces.len() as u8)
                    .min(MAX_ARGUMENT_REGISTERS);
                for piece in &classified.pieces {
                    carrier_indices.push(push_carrier(
                        &mut clif_parameters,
                        Some(source_index),
                        Some(piece.index),
                        piece.offset,
                        piece.valid_bytes,
                        AbiClass::Integer,
                        AbiCarrier::I64,
                        IntegerExtension::None,
                        NativePurpose::Normal,
                    )?);
                }
            }
            PassingMode::Memory => {
                gp_used = gp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                carrier_indices.push(push_carrier(
                    &mut clif_parameters,
                    Some(source_index),
                    None,
                    0,
                    8,
                    AbiClass::Memory,
                    AbiCarrier::I64,
                    IntegerExtension::None,
                    NativePurpose::IndirectArgument,
                )?);
            }
            PassingMode::Void => unreachable!(),
        }
        planned_parameters.push(NativeParameterPlan {
            source_index,
            ty: parameter.ty,
            classified,
            carrier_indices,
        });
    }

    let mut clif_results = Vec::new();
    let result = match result_classified.passing {
        PassingMode::Void => NativeResultPlan::Void,
        PassingMode::Scalar => {
            let scalar = boundary_scalar(types, signature.result.ty, config, "return")?;
            let carrier_index = push_carrier(
                &mut clif_results,
                None,
                None,
                0,
                scalar_size(scalar),
                scalar_class(scalar),
                scalar_carrier(scalar),
                scalar_extension(scalar),
                NativePurpose::Normal,
            )?;
            NativeResultPlan::Scalar {
                ty: signature.result.ty,
                carrier_index,
            }
        }
        PassingMode::Registers => {
            if result_classified.pieces.len() > MAX_RESULT_REGISTERS {
                return Err(AbiError::new(
                    "CCC3503",
                    "arm64 aggregate result exceeds the register result limit",
                ));
            }
            let homogeneous = is_homogeneous(&result_classified);
            let mut carrier_indices = Vec::new();
            for piece in &result_classified.pieces {
                carrier_indices.push(push_carrier(
                    &mut clif_results,
                    None,
                    Some(piece.index),
                    piece.offset,
                    piece.valid_bytes,
                    piece.class,
                    if homogeneous {
                        float_piece_carrier(piece)?
                    } else {
                        AbiCarrier::I64
                    },
                    IntegerExtension::None,
                    NativePurpose::Normal,
                )?);
            }
            NativeResultPlan::RegisterAggregate {
                classified: result_classified,
                carrier_indices,
            }
        }
        PassingMode::Memory => NativeResultPlan::Indirect {
            classified: result_classified,
            sret_parameter_index: 0,
        },
    };

    Ok(NativeBoundaryPlan {
        calling_convention: config.target.abi.calling_convention(),
        parameters: planned_parameters,
        result,
        clif_parameters,
        clif_results,
        variadic: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn push_carrier(
    carriers: &mut Vec<NativeCarrierPlan>,
    source_index: Option<u32>,
    piece_index: Option<u8>,
    source_offset: u64,
    valid_bytes: u8,
    class: AbiClass,
    carrier: AbiCarrier,
    extension: IntegerExtension,
    purpose: NativePurpose,
) -> Result<u32, AbiError> {
    let index = u32::try_from(carriers.len())
        .map_err(|_| AbiError::new("CCC3503", "ABI carrier count overflow"))?;
    carriers.push(NativeCarrierPlan {
        abi_param_index: index,
        source_index,
        piece_index,
        source_offset,
        valid_bytes,
        class,
        carrier,
        extension,
        purpose,
    });
    Ok(index)
}

fn classify(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<ClassifiedType, AbiError> {
    if types.builtin_type(ty) == Some(BuiltinType::Void) {
        return Ok(ClassifiedType {
            ty,
            size: 0,
            align: 1,
            classes: Vec::new(),
            pieces: Vec::new(),
            passing: PassingMode::Void,
        });
    }
    reject_binary128_recursive(types, ty, config, boundary)?;
    let layout = types.layout_of(ty, config).map_err(|error| {
        AbiError::new(
            "CCC3502",
            format!("type `{}` has no ABI layout: {error}", types.display(ty)),
        )
    })?;
    if !is_aggregate(types, ty) {
        let scalar = boundary_scalar(types, ty, config, boundary)?;
        let class = scalar_class(scalar);
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: vec![class],
            pieces: vec![AbiPiece {
                index: 0,
                offset: 0,
                valid_bytes: scalar_size(scalar),
                class,
            }],
            passing: PassingMode::Scalar,
        });
    }
    if let Some(members) = homogeneous_members(types, ty, config, 0)?
        && (1..=4).contains(&members.len())
    {
        let class = AbiClass::Sse;
        let pieces = members
            .iter()
            .enumerate()
            .map(|(index, member)| AbiPiece {
                index: index as u8,
                offset: member.offset,
                valid_bytes: member.size,
                class,
            })
            .collect::<Vec<_>>();
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: vec![class; pieces.len()],
            pieces,
            passing: PassingMode::Registers,
        });
    }
    if layout.size > 16 {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    let pieces = (0..layout.size.div_ceil(8))
        .map(|index| AbiPiece {
            index: index as u8,
            offset: index * 8,
            valid_bytes: layout.size.saturating_sub(index * 8).min(8) as u8,
            class: AbiClass::Integer,
        })
        .collect::<Vec<_>>();
    Ok(ClassifiedType {
        ty,
        size: layout.size,
        align: layout.align,
        classes: vec![AbiClass::Integer; pieces.len()],
        pieces,
        passing: PassingMode::Registers,
    })
}

#[derive(Clone, Copy)]
struct HomogeneousMember {
    offset: u64,
    size: u8,
    builtin: BuiltinType,
}

fn homogeneous_members(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    base: u64,
) -> Result<Option<Vec<HomogeneousMember>>, AbiError> {
    let result = match types.try_kind(ty) {
        Some(TypeKind::Builtin(builtin @ (BuiltinType::Float | BuiltinType::Double))) => {
            let size = if *builtin == BuiltinType::Float { 4 } else { 8 };
            Some(vec![HomogeneousMember {
                offset: base,
                size,
                builtin: *builtin,
            }])
        }
        Some(TypeKind::Array(array)) => {
            let ArrayLength::Constant(length) = array.length else {
                return Ok(None);
            };
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("array has no ABI layout: {error}"))
            })?;
            let LayoutShape::Array { stride, .. } = layout.shape else {
                return Ok(None);
            };
            let mut members = Vec::new();
            for index in 0..length {
                let Some(element) =
                    homogeneous_members(types, array.element.ty, config, base + index * stride)?
                else {
                    return Ok(None);
                };
                members.extend(element);
            }
            Some(members)
        }
        Some(TypeKind::Record(id)) => {
            let definition = types
                .record(*id)
                .ok_or_else(|| AbiError::new("CCC3502", format!("record {} is unknown", id.0)))?;
            if definition.kind != RecordKind::Struct {
                return Ok(None);
            }
            let fields = definition.fields.as_ref().ok_or_else(|| {
                AbiError::new(
                    "CCC3502",
                    format!("type `{}` is incomplete", types.display(ty)),
                )
            })?;
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("record has no ABI layout: {error}"))
            })?;
            let LayoutShape::Record(record_layout) = layout.shape else {
                return Ok(None);
            };
            let mut members = Vec::new();
            for field_layout in &record_layout.fields {
                if field_layout.bitfield.is_some() {
                    return Ok(None);
                }
                let field = &fields[field_layout.index];
                let Some(field_members) =
                    homogeneous_members(types, field.ty.ty, config, base + field_layout.offset)?
                else {
                    return Ok(None);
                };
                members.extend(field_members);
            }
            Some(members)
        }
        _ => None,
    };
    let Some(members) = result else {
        return Ok(None);
    };
    if members.is_empty()
        || members.len() > 4
        || members
            .iter()
            .any(|member| member.builtin != members[0].builtin)
    {
        Ok(None)
    } else {
        Ok(Some(members))
    }
}

fn is_homogeneous(classified: &ClassifiedType) -> bool {
    classified.passing == PassingMode::Registers
        && !classified.pieces.is_empty()
        && classified
            .pieces
            .iter()
            .all(|piece| piece.class == AbiClass::Sse)
}

fn memory_classification(ty: TypeId, size: u64, align: u64) -> ClassifiedType {
    ClassifiedType {
        ty,
        size,
        align,
        classes: vec![AbiClass::Memory],
        pieces: Vec::new(),
        passing: PassingMode::Memory,
    }
}

fn boundary_scalar(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    match types.try_kind(ty) {
        Some(TypeKind::Builtin(BuiltinType::Float)) => Ok(AbiScalar::Float32),
        Some(TypeKind::Builtin(BuiltinType::Double)) => Ok(AbiScalar::Float64),
        Some(TypeKind::Builtin(BuiltinType::LongDouble))
            if config.target.data_layout.long_double_width == 64 =>
        {
            Ok(AbiScalar::Float64)
        }
        Some(TypeKind::Builtin(BuiltinType::LongDouble)) => Err(AbiError::new(
            "CCC3509",
            format!(
                "binary128 `long double` {boundary} type `{}` has no enabled transport",
                types.display(ty)
            ),
        )),
        Some(TypeKind::Builtin(BuiltinType::Void)) => Err(AbiError::new(
            "CCC3507",
            "`void` has no scalar ABI representation",
        )),
        Some(TypeKind::Builtin(builtin)) if builtin.is_integer() => {
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("integer has no ABI layout: {error}"))
            })?;
            let bits = (layout.size * 8) as u8;
            let signed = match builtin {
                BuiltinType::Char => config.target.data_layout.char_is_signed,
                BuiltinType::SignedChar
                | BuiltinType::Short
                | BuiltinType::Int
                | BuiltinType::Long
                | BuiltinType::LongLong => true,
                _ => false,
            };
            Ok(if signed {
                AbiScalar::SignedInteger { bits }
            } else {
                AbiScalar::UnsignedInteger { bits }
            })
        }
        Some(TypeKind::Pointer(_)) => Ok(AbiScalar::Pointer { bits: 64 }),
        Some(TypeKind::Enum(id)) => {
            let underlying = types
                .enumeration(*id)
                .and_then(|definition| definition.body.as_ref())
                .ok_or_else(|| {
                    AbiError::new(
                        "CCC3502",
                        format!("type `{}` is incomplete", types.display(ty)),
                    )
                })?
                .underlying;
            boundary_scalar(types, underlying, config, boundary)
        }
        _ => Err(AbiError::new(
            "CCC3508",
            format!(
                "aggregate {boundary} type `{}` has no scalar ABI representation",
                types.display(ty)
            ),
        )),
    }
}

fn scalar_class(scalar: AbiScalar) -> AbiClass {
    match scalar {
        AbiScalar::Float32 | AbiScalar::Float64 => AbiClass::Sse,
        _ => AbiClass::Integer,
    }
}

fn scalar_carrier(scalar: AbiScalar) -> AbiCarrier {
    match scalar {
        AbiScalar::SignedInteger { bits }
        | AbiScalar::UnsignedInteger { bits }
        | AbiScalar::Pointer { bits } => match bits {
            8 => AbiCarrier::I8,
            16 => AbiCarrier::I16,
            32 => AbiCarrier::I32,
            64 => AbiCarrier::I64,
            _ => unreachable!("unsupported scalar width"),
        },
        AbiScalar::Float32 => AbiCarrier::F32,
        AbiScalar::Float64 => AbiCarrier::F64,
    }
}

fn scalar_size(scalar: AbiScalar) -> u8 {
    match scalar {
        AbiScalar::SignedInteger { bits }
        | AbiScalar::UnsignedInteger { bits }
        | AbiScalar::Pointer { bits } => bits / 8,
        AbiScalar::Float32 => 4,
        AbiScalar::Float64 => 8,
    }
}

fn scalar_extension(scalar: AbiScalar) -> IntegerExtension {
    match scalar {
        AbiScalar::SignedInteger { bits } if bits < 32 => IntegerExtension::Signed,
        AbiScalar::UnsignedInteger { bits } if bits < 32 => IntegerExtension::Unsigned,
        _ => IntegerExtension::None,
    }
}

fn float_piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match piece.valid_bytes {
        4 => Ok(AbiCarrier::F32),
        8 => Ok(AbiCarrier::F64),
        _ => Err(AbiError::new(
            "CCC3503",
            "homogeneous arm64 aggregate has an invalid element width",
        )),
    }
}

fn reject_binary128_recursive(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<(), AbiError> {
    if types.builtin_type(ty) == Some(BuiltinType::LongDouble)
        && config.target.data_layout.long_double_width == 128
    {
        return Err(AbiError::new(
            "CCC3509",
            format!(
                "binary128 `long double` {boundary} type `{}` has no enabled transport",
                types.display(ty)
            ),
        ));
    }
    match types.try_kind(ty) {
        Some(TypeKind::Array(array)) => {
            reject_binary128_recursive(types, array.element.ty, config, boundary)
        }
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
            for field in fields {
                reject_binary128_recursive(types, field.ty.ty, config, boundary)?;
            }
            Ok(())
        }
        Some(TypeKind::Function(_)) => Err(AbiError::new(
            "CCC3501",
            format!(
                "function {boundary} type `{}` must be adjusted to a pointer before ABI planning",
                types.display(ty)
            ),
        )),
        Some(_) => Ok(()),
        None => Err(AbiError::new(
            "CCC3501",
            format!("unknown type {} has no ABI plan", ty.index()),
        )),
    }
}

fn is_aggregate(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.try_kind(ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    )
}

fn reject_array_boundary(types: &TypeStore, ty: TypeId, boundary: &str) -> Result<(), AbiError> {
    if matches!(types.try_kind(ty), Some(TypeKind::Array(_))) {
        return Err(AbiError::new(
            "CCC3501",
            format!(
                "array {boundary} type `{}` must be adjusted before ABI planning",
                types.display(ty)
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ccc_types::{Field, QualifiedType};

    use super::*;

    fn record(types: &mut TypeStore, fields: Vec<Field>) -> TypeId {
        let (id, ty) = types.declare_record(RecordKind::Struct, None);
        types.complete_record(id, fields).unwrap();
        ty
    }

    #[test]
    fn aapcs64_classifies_hfa_integer_and_indirect_aggregates() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let h = record(
            &mut types,
            vec![
                Field::named("a", TypeId::DOUBLE),
                Field::named("b", TypeId::DOUBLE),
                Field::named("c", TypeId::DOUBLE),
            ],
        );
        let i = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
            ],
        );
        let l = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
                Field::named("c", TypeId::LONG),
            ],
        );
        for (ty, carriers, expected) in [(h, 3, AbiCarrier::F64), (i, 2, AbiCarrier::I64)] {
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(ty),
                vec![QualifiedType::unqualified(ty)],
            ));
            let plan = plan_function_type(&types, signature, &config).unwrap();
            assert_eq!(plan.clif_parameters.len(), carriers);
            assert!(
                plan.clif_parameters
                    .iter()
                    .all(|piece| piece.carrier == expected)
            );
        }
        let signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(l),
            vec![QualifiedType::unqualified(l)],
        ));
        let plan = plan_function_type(&types, signature, &config).unwrap();
        assert_eq!(
            plan.clif_parameters[1].purpose,
            NativePurpose::IndirectArgument
        );
        assert!(matches!(plan.result, NativeResultPlan::Indirect { .. }));
    }
}
